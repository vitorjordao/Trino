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

pub mod crash;
pub mod net;
pub mod package;
pub mod quality;
#[cfg(feature = "steam")]
pub mod steam;
pub mod store;

pub use package::PackageInfo;
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
    /// Where saves and settings go (`None`: `<target>/save`). A package
    /// sets the user's data folder; `TRINO_SAVE_DIR` overrides either.
    pub save_dir: Option<PathBuf>,
    /// The package this executable is part of ([`package::detect`]): its
    /// assets are pre-baked beside it.
    pub package: Option<PackageInfo>,
    /// The game's build (`trino_core::build_info!()` in its host's `main`):
    /// crash reports name it (a package's marker wins).
    pub build: Option<trino_core::BuildInfo>,
    /// When the renderer warms its pipelines (issue #101): `Background`
    /// by default (workers compile while the first frames present);
    /// `TRINO_PRECOMPILE=background|blocking|off` overrides it.
    pub precompile: PrecompileAtBoot,
    /// The precompile workers (`None`: `TRINO_THREADS`, else all CPUs but
    /// one, at least one).
    pub threads: Option<trino_core::ThreadSetting>,
    /// The most local players (`trino.toml` `[players] max`, issue #67):
    /// above 1 each pad after the first plays for its own player
    /// (`trino_core::PlayerSetup`). 1 by default: every device is player 1.
    pub players: u8,
}

/// Start the boot warm-up (issue #101): `default` unless
/// `TRINO_PRECOMPILE=background|blocking|off` says otherwise. A blocking
/// one prints `TRINO_PRECOMPILE:` and saves the pipeline cache.
pub fn boot_precompile(renderer: &mut PcRenderer, default: PrecompileAtBoot) {
    let mode = std::env::var("TRINO_PRECOMPILE")
        .ok()
        .and_then(|v| PrecompileAtBoot::parse(&v))
        .unwrap_or(default);
    let request = trino_core::PrecompileRequest::EVERYTHING;
    match mode {
        PrecompileAtBoot::Background => {
            renderer.precompile(&request);
        }
        PrecompileAtBoot::Blocking => {
            let s = renderer.precompile(&request.blocking());
            println!(
                "TRINO_PRECOMPILE: {} passes in {} ms on {} threads",
                s.total,
                s.elapsed_us / 1000,
                s.threads
            );
            if let Err(e) = renderer.save_pipeline_cache() {
                eprintln!("[trino] pipeline cache: {e}");
            }
        }
        PrecompileAtBoot::Off => {}
    }
}

/// [`RunConfig::precompile`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrecompileAtBoot {
    #[default]
    Background,
    /// Before the first frame (a longer start, never a hitch).
    Blocking,
    /// Only what the game asks for (`Renderer::precompile`).
    Off,
}

impl PrecompileAtBoot {
    pub fn parse(s: &str) -> Option<PrecompileAtBoot> {
        match s.trim() {
            "background" | "auto" => Some(PrecompileAtBoot::Background),
            "blocking" | "boot" => Some(PrecompileAtBoot::Blocking),
            "off" | "0" | "manual" => Some(PrecompileAtBoot::Off),
            _ => None,
        }
    }
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
            save_dir: None,
            package: None,
            build: None,
            precompile: PrecompileAtBoot::default(),
            threads: None,
            players: 1,
        }
    }

    /// Who a crash report names: the package's marker, else the build.
    pub fn crash_info(&self) -> crash::CrashInfo {
        let profile = self
            .default_profile
            .map(|p| format!("{p:?}").to_lowercase())
            .unwrap_or_else(|| "n64".into());
        match (&self.package, &self.build) {
            (Some(p), _) => crash::CrashInfo {
                game: p.name.clone(),
                version: p.version.clone(),
                commit: p.commit.clone(),
                date: p.date.clone(),
                profile,
            },
            (None, Some(b)) => crash::CrashInfo {
                game: b.name.into(),
                version: b.version.into(),
                commit: b.commit.into(),
                date: b.date.into(),
                profile,
            },
            (None, None) => crash::CrashInfo {
                game: self.save_name.clone(),
                version: "?".into(),
                commit: "dev".into(),
                date: String::new(),
                profile,
            },
        }
    }

    /// Where crash reports may be sent: the package's `crash_upload`, else
    /// `TRINO_CRASH_UPLOAD` (a local collector while developing).
    pub fn crash_endpoint(&self) -> Option<String> {
        self.package
            .as_ref()
            .and_then(|p| p.crash_upload.clone())
            .or_else(|| std::env::var("TRINO_CRASH_UPLOAD").ok())
            .filter(|u| !u.is_empty())
    }

    /// The player's answer to "send crash reports?" (`<save_name>.crash`).
    pub fn crash_file(&self) -> PathBuf {
        self.save_file().with_extension("crash")
    }

    /// Run as `package` says (what [`run`] does when the executable sits in
    /// a package): its title, its baked assets, its profile, the user's
    /// data folder for saves (`TRINO_SAVE_DIR` wins).
    pub fn with_package(mut self, package: PackageInfo) -> Self {
        if !package.title.is_empty() {
            self.title = package.title.clone();
        }
        self.default_profile = package.profile().or(Some(SimProfile::Native));
        self.threads = package.threads.or(self.threads);
        self.save_dir =
            package::user_data_dir(&package.save_dir).or_else(|| Some(package.dir.join("save")));
        self.package = Some(package);
        self.with_env_save_dir()
    }

    fn with_env_save_dir(mut self) -> Self {
        if let Some(d) = std::env::var_os("TRINO_SAVE_DIR").filter(|d| !d.is_empty()) {
            self.save_dir = Some(PathBuf::from(d));
        }
        self
    }

    /// The folder saves, settings, achievements and crash reports go to.
    pub fn save_dir(&self) -> PathBuf {
        self.save_dir
            .clone()
            .unwrap_or_else(|| self.target.join("save"))
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn baked_dir(&self) -> PathBuf {
        match &self.package {
            Some(p) => p.assets_dir(),
            None => self.target.join("assets/pc"),
        }
    }

    pub fn save_file(&self) -> PathBuf {
        self.save_dir().join(format!("{}.sav", self.save_name))
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

    /// The player's accessibility settings (`<save_name>.access`, plain
    /// text, `key=value` lines: `trino_platform_pc::system::access_settings_to_text`).
    pub fn access_file(&self) -> PathBuf {
        self.save_file().with_extension("access")
    }

    /// Achievements and stats (`<save_name>.achievements`, plain text:
    /// `trino_platform_pc::system::services_to_text`; issue #61).
    pub fn services_file(&self) -> PathBuf {
        self.save_file().with_extension("achievements")
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
        let tmdl = model_for(renderer, model);
        let after = tmdl.len() as u64;
        renderer.upload_mesh(id, tmdl);
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
        renderer.take_lut_blob(blob.id, &blob.bytes);
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

/// The mesh a profile draws of a baked model: its console-graded twin on
/// the console profiles (the consoles' bake grades its colors, issue #64),
/// else the model as baked.
pub fn model_for(renderer: &PcRenderer, model: trino_asset_pipeline::LoadedModel) -> Vec<u8> {
    match (renderer.profile(), model.graded) {
        (SimProfile::N64 | SimProfile::N3ds, Some(graded)) => graded,
        _ => model.tmdl,
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

/// Load the player's accessibility settings into `platform`: `file`, then
/// `TRINO_ACCESS` over it (`cvd=deuteranopia,text_scale=200,...`, for
/// screenshots and tests). Returns whether the game's changes may be
/// persisted: not while `TRINO_ACCESS` forces settings, not on smoke runs.
pub fn load_access_settings(platform: &mut PcPlatform, file: &Path, smoke: bool) -> bool {
    let mut s = std::fs::read_to_string(file)
        .map(|t| trino_platform_pc::system::access_settings_from_text(&t))
        .unwrap_or_default();
    let forced = std::env::var("TRINO_ACCESS").ok();
    if let Some(f) = &forced {
        s = trino_core::AccessSettings::parse_text(s, f);
    }
    platform.load_access(s);
    forced.is_none() && !smoke
}

/// Persist the accessibility settings when the game changed them (and
/// `persist`, from [`load_access_settings`]).
pub fn save_access_settings(platform: &mut PcPlatform, file: &Path, persist: bool) {
    let Some(s) = platform.take_access_change() else {
        return;
    };
    if !persist {
        return;
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, trino_platform_pc::system::access_settings_to_text(&s));
}

/// Load the achievements and stats into `platform` (after the assets: the
/// names resolve through the baked table). Returns whether changes may be
/// persisted: not on smoke runs.
pub fn load_services(platform: &mut PcPlatform, file: &Path, smoke: bool) -> bool {
    if smoke {
        return false;
    }
    let table = trino_core::AchievementTable::from_platform(platform);
    if let (Ok(text), Some(table)) = (std::fs::read_to_string(file), table) {
        let state = trino_platform_pc::system::services_from_text(&text, &table);
        platform.load_services(state);
    }
    true
}

/// Persist the achievements and stats when they changed (and `persist`).
pub fn save_services(platform: &mut PcPlatform, file: &Path, persist: bool) {
    let Some(state) = platform.take_services_change() else {
        return;
    };
    if !persist {
        return;
    }
    let Some(table) = trino_core::AchievementTable::from_platform(platform) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        file,
        trino_platform_pc::system::services_to_text(&state, &table),
    );
}

/// An RGBA image as a PNG (the smoke run's `TRINO_SCREENSHOT`).
pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| e.to_string())
}

/// `TRINO_SIM`, defaulting to the N64 look.
/// `TRINO_TRACE` (issue #62): record the engine's zones as `session`
/// (`pc-castle64`) when the build has `trino-core/trace` (`cargo xtask run
/// pc --trace`). `stdout` (or `1`) prints the `TRINO_TRACE:` lines; anything
/// else is a file written from scratch. `None` when unset or the build has
/// no zones (said once on stderr).
pub fn trace_from_env(session: &str) -> Option<trino_platform_pc::trace_host::TraceHost> {
    use std::io::Write as _;
    // The `tracy` feature: Tracy always listens (on demand: nothing is sent
    // until a viewer connects), with or without TRINO_TRACE.
    #[cfg(feature = "tracy")]
    if trino_platform_pc::tracy::start() {
        eprintln!("TRINO_TRACY: listening (connect the Tracy profiler)");
    }
    let target = std::env::var("TRINO_TRACE").ok()?;
    if !trino_core::trace::ENABLED {
        eprintln!(
            "TRINO_TRACE: this build has no zones (build with --features trino-core/trace, \
             or `cargo xtask run pc --trace`)"
        );
        return None;
    }
    let out: Box<dyn FnMut(&str)> = if target == "stdout" || target == "1" {
        Box::new(|l: &str| print!("{l}"))
    } else {
        let path = PathBuf::from(&target);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::File::create(&path) {
            Ok(f) => {
                let mut w = std::io::LineWriter::new(f);
                Box::new(move |l: &str| {
                    let _ = w.write_all(l.as_bytes());
                })
            }
            Err(e) => {
                eprintln!("TRINO_TRACE {}: {e}", path.display());
                return None;
            }
        }
    };
    let host = trino_platform_pc::trace_host::TraceHost::start(
        trino_core::trace::TracePlatform::Pc,
        session,
        out,
    )?;
    eprintln!("TRINO_TRACE: recording {session} into {target}");
    Some(host)
}

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
    let config = match package::detect() {
        Some(p) => config.with_package(p),
        None => config.with_env_save_dir(),
    };
    crash::install(config.crash_info(), &config.save_dir());
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
        trace: None,
    };
    host.trace = trace_from_env(&format!("pc-{}", host.config.save_name));
    event_loop.run_app(&mut host).expect("event loop error");
    if let Some(t) = host.trace.take() {
        t.finish();
    }
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
    /// Whether accessibility changes go to `<save_name>.access`.
    access_persist: bool,
    /// Whether achievements go to `<save_name>.achievements`.
    services_persist: bool,
    /// A store's services (Steam), when this run has one.
    store: Option<Box<dyn store::Store>>,
    /// Where crash reports go and may be sent.
    crash_dir: PathBuf,
    crash_endpoint: Option<String>,
    input: PcInput,
    platform: PcPlatform,
    /// The `native` profile's quality (the consoles' is fixed).
    quality: Option<QualityHost>,
    /// Serves `Platform::load_group` before every update.
    groups: GroupLoader,
    game: Box<dyn Game>,
    /// `TRINO_RECORD` / `TRINO_REPLAY` (issue #67): what each update gets.
    session: trino_platform_pc::replay_host::InputSession,
    /// `TRINO_NET` (issue #67): the session runs online.
    net: Option<net::NetHost>,
    /// The window's title, and the online status last shown in it.
    title: String,
    net_label: String,
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
    /// `TRINO_TRACE`: the engine's zones (issue #62).
    trace: Option<trino_platform_pc::trace_host::TraceHost>,
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
        // A package's icon (`icon.png` beside its marker) on the window.
        if let Some(p) = &cfg.package
            && let Ok((w, h, rgba)) = trino_asset_pipeline::decode_png(&p.dir.join("icon.png"))
            && let Ok(icon) = winit::window::Icon::from_rgba(rgba, w, h)
        {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        // The pipeline cache lives with the player's files (a package's
        // user data folder), never in the build tree.
        let options = trino_platform_pc::renderer::RendererOptions {
            pipeline_cache_dir: Some(cfg.save_dir().join("shaders")),
        };
        let mut renderer = pollster::block_on(PcRenderer::new_windowed_with(
            window.clone(),
            self.profile,
            &options,
        ))
        .expect("failed to initialize wgpu renderer");
        // `TRINO_THREADS` (read by the renderer) beats the project's value.
        if let Some(t) = cfg
            .threads
            .filter(|_| std::env::var_os("TRINO_THREADS").is_none())
        {
            renderer
                .set_precompile_threads(t.resolve(trino_platform_pc::renderer::available_cpus()));
        }
        let tier = tier_from_env();
        renderer.set_hw_tier(tier);
        let mut audio = PcAudio::new();
        let mut platform =
            PcPlatform::new(&self.profile.caps().with_tier(tier)).with_save_file(cfg.save_file());
        platform.set_second_screen(renderer.second_screen().map(|s| (s.x as u16, s.y as u16)));
        if let Some(h) = heap() {
            platform.set_heap(h);
        }
        // A package ships its assets baked; a development run bakes them.
        if cfg.package.is_none() {
            trino_asset_pipeline::bake_all(
                &cfg.assets_dir(),
                trino_asset_pipeline::Platform::Pc,
                &cfg.baked_dir(),
            )
            .unwrap_or_else(|e| panic!("asset bake failed: {e}"));
        }
        upload_assets(
            &cfg.baked_dir(),
            &mut renderer,
            &mut audio,
            &mut platform,
            None,
        )
        .unwrap_or_else(|e| panic!("failed to load baked assets: {e}"));
        boot_precompile(&mut renderer, cfg.precompile);
        let mut groups = GroupLoader::new(&cfg.baked_dir(), &renderer, &mut platform);
        if cfg.package.is_none()
            && let Some(plan) = GroupLoader::strict_plan(&cfg.assets_dir(), &renderer)
        {
            groups = groups.with_plan(plan);
            groups.check_boot();
        }
        let audio_saved = load_audio_settings(&mut audio, &cfg.audio_file());
        let access_persist = load_access_settings(
            &mut platform,
            &cfg.access_file(),
            self.smoke_frames.is_some(),
        );
        let services_persist = load_services(
            &mut platform,
            &cfg.services_file(),
            self.smoke_frames.is_some(),
        );
        let steam_app = cfg.package.as_ref().and_then(|p| p.steam_app_id);
        let manifest = cfg.package.as_ref().and_then(|p| {
            let m = p.dir.join(format!("game_actions_{}.vdf", p.steam_app_id?));
            m.is_file().then_some(m)
        });
        let store = store::connect(steam_app, manifest.as_deref(), &mut platform);
        // Crash reports: the GPU for the next one, what earlier runs left,
        // sent now if the player said yes.
        crash::set_gpu(&renderer.adapter_info());
        let crash_dir = crash::dir(&cfg.save_dir());
        let crash_endpoint = cfg.crash_endpoint();
        let consent = crash::load_consent(&cfg.crash_file());
        platform.set_crash_reports(Some(crash::status(
            &crash_dir,
            consent,
            crash_endpoint.as_deref(),
        )));
        if let Some(url) = crash_endpoint.clone()
            && self.smoke_frames.is_none()
        {
            let dir = crash_dir.clone();
            std::thread::spawn(move || crash::upload_pending(&dir, &url, consent));
        }

        #[cfg(feature = "watch")]
        let (asset_events, watcher) = {
            let (tx, rx) = std::sync::mpsc::channel();
            // A package has no masters to watch.
            let watcher: Box<dyn std::any::Any> = if cfg.package.is_some() {
                Box::new(())
            } else {
                Box::new(
                    trino_asset_pipeline::watch::watch(
                        cfg.assets_dir(),
                        trino_asset_pipeline::Platform::Pc,
                        cfg.baked_dir(),
                        move |changed| {
                            let _ = tx.send(changed);
                        },
                    )
                    .expect("failed to start asset watcher"),
                )
            };
            (rx, watcher)
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
        // `trino.toml [players] max` (the baked `net/config`, issue #67).
        let net_blob = trino_core::Platform::blob(
            &platform,
            trino_core::BlobId::from_path(trino_asset_pipeline::net::LOGICAL),
        );
        let max_players = net_blob
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|t| {
                t.lines().find_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    (k.trim() == "max_players").then(|| v.trim().parse::<u8>().ok())?
                })
            })
            .unwrap_or(cfg.players);
        let net = match net::NetHost::from_env(net_blob) {
            None => None,
            Some(Ok(n)) => Some(n),
            Some(Err(e)) => panic!("TRINO_NET: {e}"),
        };
        let make = self.make.take().expect("game created once");
        let game = make(Vec2::new(w as f32, h as f32));
        let mut session = trino_platform_pc::replay_host::InputSession::from_env(
            &cfg.save_name,
            (w as u16, h as u16),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        session.start(game.as_ref());
        window.request_redraw();
        let mut input = PcInput::new();
        if let Ok(t) = std::fs::read_to_string(cfg.input_file()) {
            input.set_all_bindings(trino_platform_pc::input::players_bindings_from_text(&t));
        }
        if max_players > 1 {
            input
                .seats_mut()
                .set_setup(trino_core::PlayerSetup::up_to(max_players));
        }
        platform.publish_input(&input);
        let mut ctx = Ctx {
            window,
            settings,
            monitor,
            renderer,
            audio,
            audio_saved,
            access_persist,
            services_persist,
            store,
            crash_dir,
            crash_endpoint,
            input,
            platform,
            quality,
            groups,
            game,
            session,
            net,
            title: cfg.title.clone(),
            net_label: String::new(),
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
            trino_platform_pc::input::players_bindings_to_text(ctx.input.all_bindings()),
        );
    }
    ctx.platform.publish_input(&ctx.input);
}

impl ApplicationHandler for Host {
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // What this run compiled starts the next one warm (Vulkan).
        if let Some(ctx) = &self.ctx
            && let Err(e) = ctx.renderer.save_pipeline_cache()
        {
            eprintln!("[trino] pipeline cache: {e}");
        }
        // A recording is written (a replay reported) when the window closes.
        if let Some(ctx) = &mut self.ctx {
            ctx.session.finish();
            if let Some(n) = &ctx.net {
                println!("{}", n.line());
            }
        }
    }

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
                // The players (issue #67): the game's seat requests, the
                // devices, then a recording over them.
                while let Some(r) = ctx.platform.players.take_request() {
                    ctx.input.seats_mut().apply(r);
                }
                let mut live = ctx.input.poll_players();
                if let Some(s) = &mut ctx.store {
                    s.input(&mut live.players[0], &mut ctx.platform);
                }
                ctx.platform.players.publish(ctx.input.seats());
                let Some((players, dt)) = ctx.session.input(&live, dt) else {
                    // A replay ran out: its verdict, its last frame, exit.
                    ctx.session.finish();
                    if let Ok(path) = std::env::var("TRINO_SCREENSHOT") {
                        let (w, h) = ctx.renderer.internal_size();
                        let rgba = ctx.renderer.read_offscreen();
                        match write_png(Path::new(&path), w, h, &rgba) {
                            Ok(()) => println!("TRINO_SCREENSHOT: {path}"),
                            Err(e) => eprintln!("TRINO_SCREENSHOT {path}: {e}"),
                        }
                    }
                    event_loop.exit();
                    return;
                };
                if ctx.session.is_replay() {
                    ctx.platform.players.publish_players(&players);
                }
                let input = players.players[0];
                let zone = trino_core::trace::Zone::begin("groups");
                ctx.groups
                    .pump(&mut ctx.renderer, &mut ctx.audio, &mut ctx.platform);
                zone.end();
                let zone = trino_core::trace::Zone::begin("update");
                if let Some(net) = &mut ctx.net {
                    // Online (issue #67): the session advances the game;
                    // this machine's devices are its seat.
                    if net.frame(ctx.game.as_mut(), &mut ctx.platform, &mut ctx.audio, &input) {
                        let p = *net.session.players();
                        ctx.session.after(ctx.game.as_ref(), &p, net.session.dt());
                    }
                    if net.label != ctx.net_label {
                        ctx.net_label = net.label.clone();
                        ctx.window
                            .set_title(&format!("{} — {}", ctx.title, ctx.net_label));
                    }
                } else {
                    ctx.game
                        .update_with(&mut ctx.platform, &input, &mut ctx.audio, dt);
                    ctx.session.after(ctx.game.as_ref(), &players, dt);
                }
                zone.end();
                let zone = trino_core::trace::Zone::begin("render");
                ctx.renderer
                    .apply_access(&trino_core::Platform::access(&ctx.platform));
                ctx.game.render(&mut ctx.renderer);
                zone.end();
                if let Some(t) = &mut self.trace {
                    t.frame();
                }
                if let Some(s) = ctx.platform.take_display_request() {
                    apply_display(ctx, self.profile, &self.config.display_file(), s);
                }
                apply_input(ctx, &self.config.input_file());
                save_audio_settings(&ctx.audio, &self.config.audio_file(), &mut ctx.audio_saved);
                save_access_settings(
                    &mut ctx.platform,
                    &self.config.access_file(),
                    ctx.access_persist,
                );
                if let Some(s) = &mut ctx.store {
                    s.pump(&mut ctx.platform);
                }
                if let Some(c) = ctx.platform.take_crash_consent() {
                    crash::save_consent(&self.config.crash_file(), c);
                    if let Some(url) = ctx.crash_endpoint.clone() {
                        let dir = ctx.crash_dir.clone();
                        std::thread::spawn(move || crash::upload_pending(&dir, &url, c));
                    }
                }
                // `TRINO_CRASH_AFTER=N`: a deliberate panic at frame N (the
                // crash report's end-to-end check).
                if let Some(n) = std::env::var("TRINO_CRASH_AFTER")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    && self.frames + 1 >= n
                {
                    panic!(
                        "TRINO_CRASH_AFTER: deliberate crash at frame {}",
                        self.frames + 1
                    );
                }
                save_services(
                    &mut ctx.platform,
                    &self.config.services_file(),
                    ctx.services_persist,
                );
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
                self.meter
                    .record_pipes(ctx.renderer.frame_times().pipelines);
                self.meter.record_post(
                    ctx.renderer.frame_times().post_passes,
                    ctx.renderer.frame_times().bloom_levels,
                );
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
                    println!("{}", self.meter.line(&self.config.save_name));
                    println!("{}", texture_line(&ctx.renderer));
                    println!("{}", audio_line(&ctx.audio));
                    if let Ok(path) = std::env::var("TRINO_SCREENSHOT") {
                        let (w, h) = ctx.renderer.internal_size();
                        let rgba = ctx.renderer.read_offscreen();
                        match write_png(Path::new(&path), w, h, &rgba) {
                            Ok(()) => println!("TRINO_SCREENSHOT: {path}"),
                            Err(e) => eprintln!("TRINO_SCREENSHOT {path}: {e}"),
                        }
                    }
                    ctx.session.finish();
                    if let Some(n) = &ctx.net {
                        println!("{}", n.line());
                    }
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

    /// The player's accessibility settings: loaded from `<save>.access`,
    /// `TRINO_ACCESS` over them (and then never written), a change saved
    /// once; never on a smoke run.
    #[test]
    fn access_settings_load_and_persist() {
        use trino_core::{AccessSettings, CvdMode, Platform};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("save/game.access");
        let mut p = PcPlatform::new(&trino_core::Caps::PC);
        assert!(
            load_access_settings(&mut p, &file, false),
            "a player run persists"
        );
        assert_eq!(p.access(), AccessSettings::DEFAULT);
        let s = AccessSettings {
            cvd: CvdMode::Grayscale,
            high_contrast: true,
            ..AccessSettings::DEFAULT
        };
        p.set_access(s);
        save_access_settings(&mut p, &file, true);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("cvd=grayscale") && text.contains("high_contrast=on"));
        let mut again = PcPlatform::new(&trino_core::Caps::PC);
        load_access_settings(&mut again, &file, false);
        assert_eq!(again.access(), s);
        // A smoke run never writes.
        let mut smoke = PcPlatform::new(&trino_core::Caps::PC);
        assert!(!load_access_settings(&mut smoke, &file, true));
        smoke.set_access(AccessSettings::DEFAULT);
        save_access_settings(&mut smoke, &file, false);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
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
        assert_eq!(c.access_file(), PathBuf::from("/p/target/save/game.access"));
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
