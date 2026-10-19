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
//! Env knobs: `TRINO_SIM=native|n64|3ds|web` (console simulation, default
//! n64; `n64x` / `3dsnew` simulate the N64 with the Expansion Pak and the New
//! 3DS: the console's look, the tier's `Caps` and `Platform::hw_tier`),
//! `TRINO_SMOKE_FRAMES=N` (exit after N frames and print a `TRINO_FRAME:`
//! line and a `TRINO_TEXTURES:` line, then `TRINO_RUN_OK frames=N` — CI smoke
//! tests), `TRINO_WINDOWED=1` (start in a window whatever the saved settings
//! say), `TRINO_BC=0` (no BC textures: decoded pixels, as on a GPU without
//! BC).
//!
//! **Display:** the game asks for [`DisplaySettings`] through
//! `Platform::set_display` (fullscreen or window, window size, pixel scale,
//! interface scale); the runner applies them, persists them next to the
//! save (`<save_name>.display`, plain text) and calls `Game::resize` when
//! the picture changes. With the `native` profile the picture is the
//! output divided by the pixel scale, its aspect kept within 4:3..21:9
//! (black bars beyond: a 32:9 monitor gets side bars, never a stretched
//! picture). The console profiles keep their fixed pictures.
//!
//! **Audio:** the project's mix (`assets/audio.toml`, the `audio/mixer`
//! asset) sets each bus's starting volume and the duck rules; the player's
//! bus volumes and mutes (`Audio::set_bus_volume`/`set_bus_muted` from the
//! game's options) persist in `<save_name>.audio` and win over the mix.
//! Smoke runs print a `TRINO_AUDIO:` line (memory, streams, underruns).
//!
//! **Quality** (the `native` profile, issue #55): presets Low…Ultra
//! ([`trino_core::QualitySettings`]), the project's own in
//! `assets/quality.ron`. The first launch calibrates (`[pc.quality]
//! default = "auto"` in `trino.toml`, the default) or takes the project's
//! preset; a Steam Deck starts on its own; the player's choice
//! (`Platform::set_quality`) persists in `<save_name>.quality`.
//! `TRINO_QUALITY=low|medium|high|ultra|auto` forces one for a run (smoke
//! runs take High); `TRINO_MSAA=0` turns MSAA off whatever the preset. The
//! `TRINO_FRAME:` line ends with the 1% lows and the quality in effect.
//! See [`quality::QualityHost`].

pub mod quality;

pub use quality::QualityHost;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

mod groups;
pub use groups::GroupLoader;

/// The heap every host on this runner reports (`Platform::memory`,
/// `TRINO_FRAME heap=`): the system allocator, counted. Off with
/// `default-features = false` for a game that brings its own allocator.
#[cfg(feature = "heap-stats")]
#[global_allocator]
pub static HEAP: trino_core::mem::Counting<std::alloc::System> =
    trino_core::mem::Counting::new(std::alloc::System);

/// [`HEAP`] for [`PcPlatform::set_heap`] (`None` without `heap-stats`).
pub fn heap() -> Option<&'static dyn trino_core::mem::HeapStats> {
    #[cfg(feature = "heap-stats")]
    {
        Some(&HEAP)
    }
    #[cfg(not(feature = "heap-stats"))]
    {
        None
    }
}

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
    pub default_profile: Option<SimProfile>,
}

impl RunConfig {
    pub fn new(title: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        RunConfig {
            title: title.into(),
            target: root.join("target"),
            root,
            save_name: "game".into(),
            default_profile: None,
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

    /// The player's quality settings (`<save_name>.quality`, plain text,
    /// `key=value` lines: `trino_platform_pc::quality::to_text`).
    pub fn quality_file(&self) -> PathBuf {
        self.save_file().with_extension("quality")
    }

    /// The player's bus volumes (`<save_name>.audio`, plain text, one line
    /// per bus: `trino_platform_pc::audio::settings_to_text`).
    pub fn audio_file(&self) -> PathBuf {
        self.save_file().with_extension("audio")
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

/// Upload (or re-upload, `only` = changed handles) the `boot` group's baked
/// assets from `baked_dir` (every asset, without groups in the manifest;
/// the others load through [`GroupLoader`]). Sounds are decoded and
/// resampled to the device rate (music that streams stays encoded); the
/// project's mix is applied. What each holds is counted in
/// `platform.memory()`.
pub fn upload_assets(
    baked_dir: &Path,
    renderer: &mut PcRenderer,
    audio: &mut PcAudio,
    platform: &mut PcPlatform,
    only: Option<&[u32]>,
) -> Result<(), String> {
    use trino_core::bundle::MemoryCategory as C;
    let assets = trino_asset_pipeline::load_boot(baked_dir, only)?;
    let target = sprite_platform(renderer);
    let clamp = |v: u64| v.min(u32::MAX as u64) as u32;
    // A re-upload replaces: take the old bytes off, count the new.
    let count = |p: &mut PcPlatform, c: C, before: u64, after: u64| {
        p.memory_mut().sub(c, clamp(before));
        p.memory_mut().add(c, clamp(after));
    };
    for sprite in assets.sprites {
        let id = trino_core::SpriteId(sprite.id);
        let before = renderer.sprite_bytes(id);
        upload_sprite(renderer, &sprite, target);
        count(platform, C::Textures, before, renderer.sprite_bytes(id));
    }
    for (list, music) in [(assets.sounds, false), (assets.music, true)] {
        for sound in list {
            let before = audio.clip_bytes(sound.id, music) as u64;
            let up = if music {
                audio.upload_music_blob(trino_core::MusicId(sound.id), &sound.blob)
            } else {
                audio.upload_sound_blob(trino_core::SoundId(sound.id), &sound.blob)
            };
            up.map_err(|e| format!("`{}`: {e}", sound.logical))?;
            let after = audio.clip_bytes(sound.id, music) as u64;
            count(platform, C::Audio, before, after);
        }
    }
    if let Some(mix) = &assets.mixer {
        let cfg = trino_core::MixerConfig::parse(mix).map_err(|e| format!("audio/mixer: {e}"))?;
        cfg.apply(audio);
    }
    for model in assets.models {
        let id = trino_core::ModelId(model.id);
        let before = renderer.mesh_bytes(id) as u64;
        let after = model.tmdl.len() as u64;
        renderer.upload_mesh(id, model.tmdl);
        count(platform, C::Meshes, before, after);
    }
    for font in assets.fonts {
        let id = trino_core::FontId(font.id);
        let before = renderer.font_bytes(id) as u64;
        let after = font.table.len() as u64;
        renderer.upload_font(id, font.table);
        count(platform, C::Data, before, after);
    }
    for blob in assets.blobs {
        let id = trino_core::BlobId(blob.id);
        let before = trino_core::Platform::blob(platform, id).map_or(0, |b| b.len() as u64);
        let after = blob.bytes.len() as u64;
        platform.insert_blob(id, blob.bytes);
        count(platform, C::Data, before, after);
    }
    Ok(())
}

/// Upload one baked sprite the way `target` shows it: the 3DS preview on
/// the 3DS profile, the master on the N64's, and on the PC the BCn blocks
/// where the GPU samples them, else those blocks decoded (logged once).
pub fn upload_sprite(
    renderer: &mut PcRenderer,
    sprite: &trino_asset_pipeline::LoadedSprite,
    target: trino_asset_pipeline::Platform,
) {
    let id = trino_core::SpriteId(sprite.id);
    if target == trino_asset_pipeline::Platform::Pc
        && let Some(c) = &sprite.compressed
    {
        let format = match c.format {
            trino_asset_pipeline::BcFormat::Bc1 => trino_platform_pc::CompressedFormat::Bc1,
            trino_asset_pipeline::BcFormat::Bc3 => trino_platform_pc::CompressedFormat::Bc3,
            trino_asset_pipeline::BcFormat::Bc7 => trino_platform_pc::CompressedFormat::Bc7,
        };
        if renderer.upload_sprite_bc(id, sprite.width, sprite.height, format, &c.data) {
            return;
        }
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            eprintln!(
                "[trino] BC textures are off (no GPU support, or TRINO_BC=0): uploading them decoded"
            );
        });
        renderer.upload_sprite(id, sprite.width, sprite.height, &sprite.pc_pixels());
        return;
    }
    renderer.upload_sprite(id, sprite.width, sprite.height, sprite.pixels_for(target));
}

/// The `TRINO_TEXTURES:` line: the sprite textures on the GPU (count,
/// bytes) and how many of them are block-compressed.
pub fn texture_line(renderer: &PcRenderer) -> String {
    let m = renderer.sprite_memory();
    format!(
        "TRINO_TEXTURES: textures={} bytes={} bc={} bc_bytes={} bc_supported={}",
        m.textures,
        m.bytes,
        m.compressed,
        m.compressed_bytes,
        renderer.supports_bc()
    )
}

/// Whose sprite pixels a renderer draws: the 3DS profile samples each
/// sprite's 3DS format (the bake's `preview_3ds`), every other profile the
/// master.
pub fn sprite_platform(renderer: &PcRenderer) -> trino_asset_pipeline::Platform {
    // By profile: a tier's Caps (the New 3DS's) are not the console's table.
    if renderer.profile() == SimProfile::N3ds {
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

/// `TRINO_AUDIO:` — what the audio holds and does (smoke runs, #53).
pub fn audio_line(audio: &PcAudio) -> String {
    let s = trino_core::Audio::audio_stats(audio);
    format!(
        "TRINO_AUDIO: rate={} resident_bytes={} voices={} streams={} stream_bytes={} \
         underruns={} streamed_frames={}",
        audio.sample_rate(),
        s.resident_bytes,
        s.voices,
        s.streams,
        s.stream_bytes,
        s.underruns,
        s.streamed_frames
    )
}

/// Load the player's bus volumes into `audio`; what it plays with after.
pub fn load_audio_settings(audio: &mut PcAudio, file: &Path) -> trino_core::BusSettings {
    if let Ok(t) = std::fs::read_to_string(file) {
        trino_platform_pc::audio::settings_from_text(&t).apply(audio);
    }
    audio.bus_settings()
}

/// Persist the bus volumes when the game changed them since `saved`.
pub fn save_audio_settings(audio: &PcAudio, file: &Path, saved: &mut trino_core::BusSettings) {
    let now = audio.bus_settings();
    if now == *saved {
        return;
    }
    *saved = now;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, trino_platform_pc::audio::settings_to_text(&now));
}

/// `TRINO_SIM`, defaulting to the N64 look.
pub fn profile_from_env() -> SimProfile {
    std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse(&v))
        .unwrap_or(SimProfile::N64)
}

/// `TRINO_SIM`'s hardware tier: `n64x` / `3dsnew` are the expanded tiers
/// (issue #54); anything else the base one.
pub fn tier_from_env() -> trino_core::HwTier {
    std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse_tier(&v))
        .map_or(trino_core::HwTier::Base, |(_, t)| t)
}

/// Open a window and run the game made by `make(screen)` until closed.
/// Panics with a readable message on setup errors (bad assets, no GPU).
pub fn run<G: Game + 'static>(config: RunConfig, make: impl FnOnce(Vec2) -> G + 'static) {
    let smoke_frames = std::env::var("TRINO_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let profile = std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse(&v))
        .or(config.default_profile)
        .unwrap_or(SimProfile::N64);
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
    /// The bus volumes last written to `<save_name>.audio`.
    audio_saved: trino_core::BusSettings,
    input: PcInput,
    platform: PcPlatform,
    /// The `native` profile's quality (the consoles' is fixed).
    quality: Option<QualityHost>,
    /// Serves `Platform::load_group` before every update.
    groups: GroupLoader,
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
        let tier = tier_from_env();
        renderer.set_hw_tier(tier);
        let mut audio = PcAudio::new();
        let mut platform =
            PcPlatform::new(&self.profile.caps().with_tier(tier)).with_save_file(cfg.save_file());
        platform.set_second_screen(renderer.second_screen().map(|s| (s.x as u16, s.y as u16)));
        if let Some(h) = heap() {
            platform.set_heap(h);
        }
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
        let mut groups = GroupLoader::new(&cfg.baked_dir(), &renderer, &mut platform);
        if let Some(plan) = GroupLoader::strict_plan(&cfg.assets_dir(), &renderer) {
            groups = groups.with_plan(plan);
            groups.check_boot();
        }
        let audio_saved = load_audio_settings(&mut audio, &cfg.audio_file());

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
        // The quality (the native profile): calibrated at the picture's
        // size on a first launch, before the game's first frame.
        let quality = self.profile.is_native().then(|| {
            let baked = trino_core::Platform::blob(
                &platform,
                trino_core::BlobId::from_path(trino_asset_pipeline::quality::LOGICAL),
            );
            let mut q = QualityHost::new(cfg.quality_file(), baked);
            let hz = window
                .current_monitor()
                .and_then(|m| m.refresh_rate_millihertz())
                .map(|mhz| mhz.div_ceil(1000));
            q.begin(&mut renderer, &mut platform, hz);
            q
        });
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
            audio_saved,
            input,
            platform,
            quality,
            groups,
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
                    // A reloaded mix resets the buses: the player's win.
                    ctx.audio_saved.apply(&mut ctx.audio);
                }
                let now = Instant::now();
                // Clamp dt: pauses and window drags must not teleport.
                let dt = (now - ctx.last_frame).as_secs_f32().min(0.1);
                ctx.last_frame = now;
                let input = ctx.input.poll();
                ctx.groups
                    .pump(&mut ctx.renderer, &mut ctx.audio, &mut ctx.platform);
                ctx.game
                    .update_with(&mut ctx.platform, &input, &mut ctx.audio, dt);
                ctx.game.render(&mut ctx.renderer);
                if let Some(s) = ctx.platform.take_display_request() {
                    apply_display(ctx, self.profile, &self.config.display_file(), s);
                }
                apply_input(ctx, &self.config.input_file());
                save_audio_settings(&ctx.audio, &self.config.audio_file(), &mut ctx.audio_saved);
                if let Some(q) = &mut ctx.quality {
                    q.end_frame(&mut ctx.renderer, &mut ctx.platform);
                    self.meter.set_quality(q.note(&ctx.renderer));
                }

                self.frames += 1;
                self.meter.record(
                    &trino_core::Renderer::stats(&ctx.renderer),
                    &ctx.renderer.pc_gpu_rejects(),
                );
                self.meter
                    .record_memory(&trino_core::Platform::memory(&ctx.platform));
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
                    println!("{}", self.meter.line(&self.config.save_name));
                    println!("{}", texture_line(&ctx.renderer));
                    println!("{}", audio_line(&ctx.audio));
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
        assert_eq!(
            c.quality_file(),
            PathBuf::from("/p/target/save/game.quality")
        );
        assert_eq!(c.input_file(), PathBuf::from("/p/target/save/game.input"));
        assert_eq!(c.audio_file(), PathBuf::from("/p/target/save/game.audio"));
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
