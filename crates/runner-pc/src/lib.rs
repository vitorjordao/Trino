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
//! `TRINO_SMOKE_FRAMES=N` (exit after N frames and print
//! `TRINO_RUN_OK frames=N` — CI smoke tests).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use trino_core::{Game, Input, Vec2};
use trino_platform_pc::{PcAudio, PcInput, PcPlatform, PcRenderer, SimProfile};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

/// Where the game lives and how its window looks.
#[derive(Clone, Debug)]
pub struct RunConfig {
    pub title: String,
    /// Project root: holds `assets/manifest.toml`; bakes go to
    /// `target/assets/pc`, saves to `target/save/<save_name>.sav`.
    pub root: PathBuf,
    pub save_name: String,
}

impl RunConfig {
    pub fn new(title: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        RunConfig {
            title: title.into(),
            root: root.into(),
            save_name: "game".into(),
        }
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn baked_dir(&self) -> PathBuf {
        self.root.join("target/assets/pc")
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
    for sprite in assets.sprites {
        renderer.upload_sprite(
            trino_core::SpriteId(sprite.id),
            sprite.width,
            sprite.height,
            &sprite.rgba,
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
    let mut host = Host {
        config,
        profile: profile_from_env(),
        make: Some(Box::new(move |s| Box::new(make(s)) as Box<dyn Game>)),
        ctx: None,
        frames: 0,
        smoke_frames,
    };
    event_loop.run_app(&mut host).expect("event loop error");
}

type MakeGame = Box<dyn FnOnce(Vec2) -> Box<dyn Game>>;

struct Ctx {
    window: Arc<Window>,
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
}

impl Host {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Ctx {
        let cfg = &self.config;
        let attrs = Window::default_attributes()
            .with_title(cfg.title.clone())
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let mut renderer =
            pollster::block_on(PcRenderer::new_windowed(window.clone(), self.profile))
                .expect("failed to initialize wgpu renderer");
        let mut audio = PcAudio::new();
        let mut platform = PcPlatform::new(&self.profile.caps()).with_save_file(
            cfg.root
                .join("target/save")
                .join(format!("{}.sav", cfg.save_name)),
        );
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

        let (w, h) = renderer.internal_size();
        let make = self.make.take().expect("game created once");
        let game = make(Vec2::new(w as f32, h as f32));
        window.request_redraw();
        Ctx {
            window,
            renderer,
            audio,
            input: PcInput::new(),
            platform,
            game,
            last_frame: Instant::now(),
            #[cfg(feature = "watch")]
            asset_events,
            #[cfg(feature = "watch")]
            _asset_watcher: watcher,
        }
    }
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
            WindowEvent::Resized(size) => ctx.renderer.resize(size.width, size.height),
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

                self.frames += 1;
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
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
    }
}
