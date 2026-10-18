//! Ready-made PC host for a Trino game — what a **standalone project**
//! (`cargo xtask new <name> --standalone <dir>`) uses as its `main`:
//!
//! ```ignore
//! fn main() {
//!     let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
//!     trino_runner_pc::run(trino_runner_pc::RunConfig::new("My game", root), |screen| {
//!         my_game::MyGame::new(screen)
//!     });
//! }
//! ```
//!
//! It bakes `<root>/assets` (manifest + masters) into
//! `<root>/target/assets/pc`, opens a window, and drives the game with the
//! wgpu renderer, the cpal mixer, keyboard/gamepad input and a save file —
//! the same backend `apps/pc` uses. With the `watch` feature, saving a
//! master rebakes and re-uploads it live.
//!
//! Env knobs: `TRINO_SIM=native|n64|3ds` (console simulation, default n64),
//! `TRINO_SMOKE_FRAMES=N` (exit after N frames and print a `TRINO_FRAME:`
//! line, then `TRINO_RUN_OK frames=N` — CI smoke tests), `TRINO_WINDOWED=1` (start in
//! a window whatever the saved settings say).
//!
//! **Display:** the game asks for [`DisplaySettings`] through
//! `Platform::set_display` (fullscreen or window, window size, pixel scale,
//! interface scale); the runner applies them, persists them next to the
//! save (`<save_name>.display`, plain text) and calls `Game::resize` when
//! the picture changes. With the `native` profile the picture is the
//! output divided by the pixel scale, its aspect kept within 4:3..21:9
//! (black bars beyond: a 32:9 monitor gets side bars, never a stretched
//! picture). The console profiles keep their fixed pictures.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use trino_core::{DisplayInfo, DisplaySettings, Game, Input, Renderer, Vec2};
use trino_platform_pc::{PcAudio, PcInput, PcPlatform, PcRenderer, SimProfile};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Fullscreen, Window, WindowId};

/// Where the game lives and how its window looks.
#[derive(Clone, Debug)]
pub struct RunConfig {
    pub title: String,
    /// Project root: holds `assets/manifest.toml`.
    pub root: PathBuf,
    /// Build output: bakes go to `<target>/assets/pc`, saves to
    /// `<target>/save/<save_name>.sav`. `<root>/target` by default.
    pub target: PathBuf,
    pub save_name: String,
}

impl RunConfig {
    pub fn new(title: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        RunConfig {
            title: title.into(),
            target: root.join("target"),
            root,
            save_name: "game".into(),
        }
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn baked_dir(&self) -> PathBuf {
        self.target.join("assets/pc")
    }

    pub fn save_file(&self) -> PathBuf {
        self.target
            .join("save")
            .join(format!("{}.sav", self.save_name))
    }

    /// Display settings, next to the save.
    pub fn display_file(&self) -> PathBuf {
        self.save_file().with_extension("display")
    }

    /// The input bindings (`<save_name>.input`, plain text, one line per
    /// console button).
    pub fn input_file(&self) -> PathBuf {
        self.save_file().with_extension("input")
    }
}

/// The window size for `settings` on a `monitor`: the saved size if it
/// fits, else the largest 16:9 size up to 1280×720 that leaves room for
/// the desktop around it.
pub fn window_size_for(settings: &DisplaySettings, monitor: (u32, u32)) -> (u32, u32) {
    let (mw, mh) = (monitor.0.max(640), monitor.1.max(360));
    let (w, h) = (settings.window_w as u32, settings.window_h as u32);
    if w >= 320 && h >= 240 && w <= mw && h <= mh {
        return (w, h);
    }
    let w = 1280u32.min(mw * 9 / 10);
    (w, (w * 9 / 16).min(mh * 9 / 10))
}

/// The picture for `profile` on an output: the native profile scales with
/// the output (see [`trino_platform_pc::sim::native_internal`]); console
/// profiles keep their fixed resolution. Returns `(w, h, pixel scale)`,
/// scale 0 for a fixed picture.
pub fn picture_for(profile: SimProfile, output: (u32, u32), pixel_scale: u8) -> (u32, u32, u8) {
    match profile {
        SimProfile::Native => {
            let (w, h, k) = trino_platform_pc::sim::native_internal(output, pixel_scale as u32);
            (w, h, k as u8)
        }
        _ => {
            let (w, h) = profile.internal_resolution();
            (w, h, 0)
        }
    }
}

/// Upload (or re-upload, `only` = changed handles) baked assets from
/// `baked_dir`. Sounds are resampled to the device rate.
pub fn upload_assets(
    baked_dir: &Path,
    renderer: &mut PcRenderer,
    audio: &mut PcAudio,
    platform: &mut PcPlatform,
    only: Option<&[u32]>,
) -> Result<(), String> {
    let assets = trino_asset_pipeline::load_dir(baked_dir, only)?;
    let target = sprite_platform(renderer);
    for sprite in assets.sprites {
        renderer.upload_sprite(
            trino_core::SpriteId(sprite.id),
            sprite.width,
            sprite.height,
            sprite.pixels_for(target),
        );
    }
    for sound in assets.sounds {
        let samples = resample(&sound.samples, sound.sample_rate, audio.sample_rate());
        audio.upload_sound(trino_core::SoundId(sound.id), samples);
    }
    for track in assets.music {
        let samples = resample(&track.samples, track.sample_rate, audio.sample_rate());
        audio.upload_music(trino_core::MusicId(track.id), samples);
    }
    for model in assets.models {
        renderer.upload_mesh(trino_core::ModelId(model.id), model.tmdl);
    }
    for font in assets.fonts {
        renderer.upload_font(trino_core::FontId(font.id), font.table);
    }
    for blob in assets.blobs {
        platform.insert_blob(trino_core::BlobId(blob.id), blob.bytes);
    }
    Ok(())
}

/// Whose sprite pixels a renderer draws: the 3DS profile samples each
/// sprite's 3DS format (the bake's `preview_3ds`), every other profile the
/// master.
pub fn sprite_platform(renderer: &PcRenderer) -> trino_asset_pipeline::Platform {
    use trino_core::Renderer;
    if *renderer.caps() == trino_core::Caps::N3DS {
        trino_asset_pipeline::Platform::N3ds
    } else {
        trino_asset_pipeline::Platform::Pc
    }
}

/// Linear-interpolation resampler (load-time, dev quality).
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (samples.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = (src - idx as f64) as f32;
            let a = samples[idx.min(samples.len() - 1)];
            let b = samples[(idx + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// `TRINO_SIM`, defaulting to the N64 look.
pub fn profile_from_env() -> SimProfile {
    std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse(&v))
        .unwrap_or(SimProfile::N64)
}

/// Open a window and run the game made by `make(screen)` until closed.
/// Panics with a readable message on setup errors (bad assets, no GPU).
pub fn run<G: Game + 'static>(config: RunConfig, make: impl FnOnce(Vec2) -> G + 'static) {
    let smoke_frames = std::env::var("TRINO_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let profile = profile_from_env();
    let mut host = Host {
        config,
        profile,
        make: Some(Box::new(move |s| Box::new(make(s)) as Box<dyn Game>)),
        ctx: None,
        frames: 0,
        smoke_frames,
        meter: trino_platform_pc::FrameMeter::new(profile),
    };
    event_loop.run_app(&mut host).expect("event loop error");
}

type MakeGame = Box<dyn FnOnce(Vec2) -> Box<dyn Game>>;

struct Ctx {
    window: Arc<Window>,
    settings: DisplaySettings,
    monitor: (u32, u32),
    renderer: PcRenderer,
    audio: PcAudio,
    input: PcInput,
    platform: PcPlatform,
    game: Box<dyn Game>,
    last_frame: Instant,
    #[cfg(feature = "watch")]
    asset_events: std::sync::mpsc::Receiver<Vec<u32>>,
    #[cfg(feature = "watch")]
    _asset_watcher: Box<dyn std::any::Any>,
}

struct Host {
    config: RunConfig,
    profile: SimProfile,
    make: Option<MakeGame>,
    ctx: Option<Ctx>,
    frames: u64,
    smoke_frames: Option<u64>,
    /// Frame time and the 3D split per frame (`TRINO_FRAME:` at the end
    /// of a smoke run).
    meter: trino_platform_pc::FrameMeter,
}

impl Host {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Ctx {
        let cfg = &self.config;
        let mut settings = std::fs::read_to_string(cfg.display_file())
            .map(|t| trino_platform_pc::system::display_settings_from_text(&t))
            .unwrap_or_default();
        if std::env::var_os("TRINO_WINDOWED").is_some() || self.smoke_frames.is_some() {
            settings.fullscreen = false;
        }
        let monitor = event_loop
            .primary_monitor()
            .map(|m| (m.size().width, m.size().height))
            .unwrap_or((1920, 1080));
        let (ww, wh) = window_size_for(&settings, monitor);
        let mut attrs = Window::default_attributes()
            .with_title(cfg.title.clone())
            .with_inner_size(PhysicalSize::new(ww, wh));
        if settings.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let mut renderer =
            pollster::block_on(PcRenderer::new_windowed(window.clone(), self.profile))
                .expect("failed to initialize wgpu renderer");
        // The PC's own profile multisamples its edges (TRINO_MSAA=0: off).
        renderer.set_msaa(std::env::var("TRINO_MSAA").as_deref() != Ok("0"));
        let mut audio = PcAudio::new();
        let mut platform = PcPlatform::new(&self.profile.caps()).with_save_file(cfg.save_file());
        platform.set_second_screen(renderer.second_screen().map(|s| (s.x as u16, s.y as u16)));
        trino_asset_pipeline::bake_all(
            &cfg.assets_dir(),
            trino_asset_pipeline::Platform::Pc,
            &cfg.baked_dir(),
        )
        .unwrap_or_else(|e| panic!("asset bake failed: {e}"));
        upload_assets(
            &cfg.baked_dir(),
            &mut renderer,
            &mut audio,
            &mut platform,
            None,
        )
        .unwrap_or_else(|e| panic!("failed to load baked assets: {e}"));

        #[cfg(feature = "watch")]
        let (asset_events, watcher) = {
            let (tx, rx) = std::sync::mpsc::channel();
            let watcher = trino_asset_pipeline::watch::watch(
                cfg.assets_dir(),
                trino_asset_pipeline::Platform::Pc,
                cfg.baked_dir(),
                move |changed| {
                    let _ = tx.send(changed);
                },
            )
            .expect("failed to start asset watcher");
            (rx, Box::new(watcher) as Box<dyn std::any::Any>)
        };

        // The picture for the window as created (fullscreen may differ
        // from the requested size).
        let out = window.inner_size();
        let (w, h, _) = picture_for(self.profile, (out.width, out.height), settings.pixel_scale);
        renderer.set_internal_size(w, h);
        let make = self.make.take().expect("game created once");
        let game = make(Vec2::new(w as f32, h as f32));
        window.request_redraw();
        let mut input = PcInput::new();
        if let Ok(t) = std::fs::read_to_string(cfg.input_file()) {
            input.set_bindings(trino_platform_pc::input::bindings_from_text(&t));
        }
        platform.publish_input(&input);
        let mut ctx = Ctx {
            window,
            settings,
            monitor,
            renderer,
            audio,
            input,
            platform,
            game,
            last_frame: Instant::now(),
            #[cfg(feature = "watch")]
            asset_events,
            #[cfg(feature = "watch")]
            _asset_watcher: watcher,
        };
        publish(&mut ctx, self.profile);
        ctx
    }
}

/// Tell the game what the display looks like now.
fn publish(ctx: &mut Ctx, profile: SimProfile) {
    let out = ctx.window.inner_size();
    let (w, h, k) = picture_for(profile, (out.width, out.height), ctx.settings.pixel_scale);
    let clamp = |v: u32| v.min(u16::MAX as u32) as u16;
    ctx.platform.set_display_info(Some(DisplayInfo {
        settings: ctx.settings,
        output: (clamp(out.width), clamp(out.height)),
        monitor: (clamp(ctx.monitor.0), clamp(ctx.monitor.1)),
        screen: (clamp(w), clamp(h)),
        pixel_scale: k,
    }));
}

/// Re-fit the picture to the window; tell the game when its size changed.
fn refit(ctx: &mut Ctx, profile: SimProfile) {
    let out = ctx.window.inner_size();
    if out.width == 0 || out.height == 0 {
        return; // minimized
    }
    let (w, h, _) = picture_for(profile, (out.width, out.height), ctx.settings.pixel_scale);
    if (w, h) != ctx.renderer.internal_size() {
        ctx.renderer.set_internal_size(w, h);
        // The renderer clamps to the GPU's texture limit: tell the game the
        // size it got.
        let (w, h) = ctx.renderer.internal_size();
        ctx.game.resize(Vec2::new(w as f32, h as f32));
    }
    publish(ctx, profile);
}

/// Apply new display settings asked for by the game, and persist them.
fn apply_display(ctx: &mut Ctx, profile: SimProfile, file: &Path, s: DisplaySettings) {
    let was_fullscreen = ctx.settings.fullscreen;
    ctx.settings = s;
    if s.fullscreen != was_fullscreen {
        ctx.window.set_fullscreen(
            s.fullscreen
                .then_some(Fullscreen::Borderless(ctx.window.current_monitor())),
        );
    }
    if !s.fullscreen {
        let (w, h) = window_size_for(&s, ctx.monitor);
        let _ = ctx.window.request_inner_size(PhysicalSize::new(w, h));
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        file,
        trino_platform_pc::system::display_settings_to_text(&s),
    );
    // The window may resize later (Resized refits again); the pixel scale
    // applies now.
    refit(ctx, profile);
}

/// Apply the game's rebind/cancel/reset, persist bindings that changed
/// (a rebind completes on a later key or pad press), and tell the game.
fn apply_input(ctx: &mut Ctx, file: &Path) {
    if let Some(r) = ctx.platform.take_input_request() {
        ctx.input.apply(r);
    }
    if ctx.input.take_changed() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            file,
            trino_platform_pc::input::bindings_to_text(ctx.input.bindings()),
        );
    }
    ctx.platform.publish_input(&ctx.input);
}

impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ctx.is_none() {
            self.ctx = Some(self.init(event_loop));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(ctx) = self.ctx.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                ctx.renderer.resize(size.width, size.height);
                refit(ctx, self.profile);
            }
            ref e if ctx.input.handle_window_event(e, &ctx.renderer) => {}
            WindowEvent::RedrawRequested => {
                #[cfg(feature = "watch")]
                while let Ok(changed) = ctx.asset_events.try_recv() {
                    match upload_assets(
                        &self.config.baked_dir(),
                        &mut ctx.renderer,
                        &mut ctx.audio,
                        &mut ctx.platform,
                        Some(&changed),
                    ) {
                        Ok(()) => println!("assets reloaded: {} item(s)", changed.len()),
                        Err(e) => eprintln!("asset reload failed: {e}"),
                    }
                }
                let now = Instant::now();
                // Clamp dt: pauses and window drags must not teleport.
                let dt = (now - ctx.last_frame).as_secs_f32().min(0.1);
                ctx.last_frame = now;
                let input = ctx.input.poll();
                ctx.game
                    .update_with(&mut ctx.platform, &input, &mut ctx.audio, dt);
                ctx.game.render(&mut ctx.renderer);
                if let Some(s) = ctx.platform.take_display_request() {
                    apply_display(ctx, self.profile, &self.config.display_file(), s);
                }
                apply_input(ctx, &self.config.input_file());

                self.frames += 1;
                self.meter.record(
                    &trino_core::Renderer::stats(&ctx.renderer),
                    &ctx.renderer.pc_gpu_rejects(),
                );
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
                    println!("{}", self.meter.line(&self.config.save_name));
                    println!("TRINO_RUN_OK frames={}", self.frames);
                    event_loop.exit();
                    return;
                }
                ctx.window.request_redraw();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_and_keeps_identity() {
        let s = [0.0, 1.0, 2.0, 3.0];
        assert_eq!(resample(&s, 100, 100), s);
        assert_eq!(resample(&s, 200, 100), vec![0.0, 2.0]);
    }

    #[test]
    fn config_paths() {
        let c = RunConfig::new("t", "/p");
        assert_eq!(c.assets_dir(), PathBuf::from("/p/assets"));
        assert_eq!(c.baked_dir(), PathBuf::from("/p/target/assets/pc"));
        assert_eq!(
            c.display_file(),
            PathBuf::from("/p/target/save/game.display")
        );
        assert_eq!(c.input_file(), PathBuf::from("/p/target/save/game.input"));
    }

    #[test]
    fn window_and_picture_sizes() {
        let d = DisplaySettings::DEFAULT;
        assert_eq!(window_size_for(&d, (1920, 1080)), (1280, 720));
        assert_eq!(window_size_for(&d, (1366, 768)), (1229, 691));
        let s = DisplaySettings {
            window_w: 1600,
            window_h: 900,
            ..d
        };
        assert_eq!(window_size_for(&s, (1920, 1080)), (1600, 900));
        // A saved size bigger than this monitor falls back.
        assert_eq!(window_size_for(&s, (1280, 800)), (1152, 648));
        assert_eq!(picture_for(SimProfile::N64, (1920, 1080), 2), (320, 240, 0));
        assert_eq!(
            picture_for(SimProfile::Native, (1920, 1080), 0),
            (640, 360, 3)
        );
    }
}
