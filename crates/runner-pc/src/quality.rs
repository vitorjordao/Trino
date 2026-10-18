//! The quality a PC host runs with (issue #55, ADR 0023): the start
//! ([`trino_platform_pc::quality::start`]), the first launch's calibration,
//! the game's requests, the frame cap and the player's file. Shared by
//! [`crate::run`] and the repo's own `apps/pc`.
//!
//! A smoke run (`TRINO_SMOKE_FRAMES`) is deterministic: High unless
//! `TRINO_QUALITY` says otherwise, no saved file read or written, no
//! calibration unless asked for (`TRINO_QUALITY=auto`).
//! `TRINO_QUALITY_BENCH=1` (`cargo xtask quality bench`) calibrates, prints
//! the `TRINO_QUALITY: bench` line and exits before the game's first
//! frame.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trino_core::{
    DynRes, FrameLimit, PresetTable, QualityInfo, QualityPreset, QualitySettings, QualitySource,
};
use trino_platform_pc::frame_report::QualityNote;
use trino_platform_pc::pacing::FrameLimiter;
use trino_platform_pc::quality::{self, QualityRequest};
use trino_platform_pc::{PcPlatform, PcRenderer};

/// The calibration's budget (the acceptance bar is 3 s).
pub const BENCH_BUDGET: Duration = Duration::from_secs(2);

pub struct QualityHost {
    /// `<save_name>.quality`.
    file: PathBuf,
    table: PresetTable,
    settings: QualitySettings,
    source: QualitySource,
    detected: Option<QualityPreset>,
    handheld: bool,
    /// Calibrate before the first frame.
    bench: bool,
    /// Write the settings once known (the first launch's pick, a Deck).
    save: bool,
    smoke: bool,
    limiter: FrameLimiter,
    last: Option<Instant>,
    /// `TRINO_MSAA=0`: no MSAA whatever the preset says.
    msaa_off: bool,
    /// `TRINO_QUALITY_BENCH=1`: calibrate, report, exit.
    bench_only: bool,
}

impl QualityHost {
    /// The start from the environment, the player's `file` and the baked
    /// `quality/presets` blob (`None`: the built-in presets, `auto`).
    pub fn new(file: PathBuf, baked: Option<&[u8]>) -> QualityHost {
        let (table, default) = quality::baked(baked);
        let smoke = std::env::var_os("TRINO_SMOKE_FRAMES").is_some();
        let forced = std::env::var("TRINO_QUALITY")
            .ok()
            .or_else(|| smoke.then(|| "high".into()));
        let saved = if smoke {
            None
        } else {
            std::fs::read_to_string(&file).ok()
        };
        let bench_only = std::env::var_os("TRINO_QUALITY_BENCH").is_some();
        let handheld = quality::is_steam_deck();
        let machine = if handheld {
            quality::Machine::SteamDeck
        } else {
            quality::Machine::Desktop
        };
        let start = quality::start(
            forced.as_deref(),
            saved.as_deref(),
            machine,
            &default,
            &table,
        );
        QualityHost {
            file,
            table,
            settings: start.settings,
            source: start.source,
            detected: None,
            handheld,
            bench: start.bench || bench_only,
            save: start.save && !smoke && !bench_only,
            smoke,
            limiter: FrameLimiter::new(start.settings.frame_limit),
            last: None,
            msaa_off: std::env::var("TRINO_MSAA").as_deref() == Ok("0"),
            bench_only,
        }
    }

    /// Before the first frame: the display's rate, the calibration when
    /// due, the settings applied and published, and saved when they are
    /// the first launch's.
    pub fn begin(
        &mut self,
        renderer: &mut PcRenderer,
        platform: &mut PcPlatform,
        refresh_hz: Option<u32>,
    ) {
        if let Some(hz) = refresh_hz {
            renderer.set_refresh_hz(hz);
        }
        platform.set_quality_presets(self.table);
        if self.bench {
            self.bench = false;
            let pick = self.calibrate(renderer, refresh_hz.unwrap_or(60));
            self.settings = self.table.get(pick);
            if self.bench_only {
                use std::io::Write as _;
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
        }
        self.apply(renderer);
        if self.save {
            self.persist();
        }
        self.publish(renderer, platform);
        println!(
            "TRINO_QUALITY: preset={} source={} scale={:.2} gpu={} adapter={}",
            self.settings.preset.key(),
            self.source.key(),
            renderer.render_scale(),
            renderer.gpu_class().key(),
            renderer.adapter_info()
        );
    }

    /// After the game's frame: its requests, the frame cap's wait, the
    /// frame's interval for dynamic resolution, and what the game sees.
    pub fn end_frame(&mut self, renderer: &mut PcRenderer, platform: &mut PcPlatform) {
        match platform.take_quality_request() {
            Some(QualityRequest::Set(q)) => {
                self.settings = QualitySettings {
                    preset: self.table.classify(&q),
                    ..q
                };
                self.source = QualitySource::Saved;
                self.apply(renderer);
                self.persist();
            }
            Some(QualityRequest::Detect) => {
                let limit = self.settings.frame_limit;
                let pick = self.calibrate(renderer, renderer.refresh_hz());
                self.settings = QualitySettings {
                    frame_limit: limit,
                    ..self.table.get(pick)
                };
                self.apply(renderer);
                self.persist();
                // The bench's frames aren't the game's.
                self.last = None;
            }
            None => {}
        }
        self.limiter.wait();
        let now = Instant::now();
        if let Some(t) = self.last {
            renderer
                .report_frame_time(now.duration_since(t).as_micros().min(u32::MAX as u128) as u32);
        }
        self.last = Some(now);
        self.publish(renderer, platform);
    }

    /// What `TRINO_FRAME` says about the quality.
    pub fn note(&self, renderer: &PcRenderer) -> QualityNote {
        QualityNote {
            preset: self.settings.preset,
            scale: renderer.render_scale(),
            source: self.source,
            detected: self.detected,
            gpu_us: renderer.gpu_frame_us(),
        }
    }

    pub fn settings(&self) -> QualitySettings {
        self.settings
    }

    fn calibrate(&mut self, renderer: &mut PcRenderer, hz: u32) -> QualityPreset {
        let target = DynRes::DEFAULT.target_for(FrameLimit::VSync.interval_us(hz));
        let cal = renderer.calibrate(&self.table, target, BENCH_BUDGET);
        let (w, h) = renderer.internal_size();
        println!(
            "TRINO_QUALITY: bench {} picture={w}x{h} adapter={}",
            cal.summary(),
            renderer.adapter_info()
        );
        self.detected = Some(cal.pick);
        self.source = QualitySource::Detected;
        cal.pick
    }

    fn apply(&mut self, renderer: &mut PcRenderer) {
        let mut q = self.settings;
        if self.msaa_off {
            q.msaa = 1;
        }
        renderer.apply_quality(&q);
        self.limiter.set(q.frame_limit);
    }

    fn persist(&self) {
        if self.smoke {
            return;
        }
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.file, quality::to_text(&self.settings));
    }

    fn publish(&self, renderer: &PcRenderer, platform: &mut PcPlatform) {
        platform.set_quality_info(Some(QualityInfo {
            settings: self.settings,
            scale: renderer.render_scale(),
            detected: self.detected,
            source: self.source,
            gpu: renderer.gpu_class(),
            handheld: self.handheld,
            gpu_us: renderer.gpu_frame_us(),
        }));
    }
}
