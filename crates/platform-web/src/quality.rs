//! The quality a page runs with (issue #55, ADR 0022): the PC's presets,
//! start and requests ([`trino_render_wgpu::quality`]) on the Web.
//!
//! - **Start:** `?quality=<preset>|auto` forced for the run, else the
//!   player's choice (`localStorage` `trino-quality:<save_name>`, the PC's
//!   `.quality` text), else a phone's or a tablet's browser the project's
//!   Low ([`crate::device`]), else the project's default.
//! - **Calibration (`auto`, "Detect again"):** a page can't wait for its
//!   GPU, so the bench draws one frame per animation frame, presented, and
//!   times the intervals between them. Under the browser's VSync a frame
//!   that holds 60 fps comes one refresh after the last, one that misses
//!   two: the target is 1.25 refreshes at 60 Hz. At most
//!   [`BENCH_BUDGET`], before the game's first frame.
//! - **Frame cap:** an animation frame comes per refresh; a cap below it
//!   skips frames (40 on a 60 Hz screen alternates one and two refreshes).
//! - **Dynamic resolution** follows the game frames' intervals.
//!
//! The self-test and `?frames=N` smoke runs are deterministic: High unless
//! `?quality=` says otherwise, nothing read or stored.

use std::time::Duration;

use trino_core::{
    FrameLimit, PresetTable, QualityInfo, QualityPreset, QualitySettings, QualitySource,
};
use trino_render_wgpu::quality::{self, Machine, QualityRequest};
use trino_render_wgpu::{Bench, FrameMeter, PcRenderer, QualityNote};

use crate::WebPlatform;

/// The calibration's budget (the acceptance bar is 3 s).
pub const BENCH_BUDGET: Duration = Duration::from_secs(2);
/// A bench frame's interval that still holds 60 fps under VSync, µs.
pub const BENCH_TARGET_US: u32 = 20_833;

pub(crate) struct WebQuality {
    /// `trino-quality:<save_name>`.
    key: String,
    /// Read and store the player's choice (not in a self-test or a smoke
    /// run).
    keep: bool,
    forced: Option<String>,
    machine: Machine,
    msaa_off: bool,
    table: PresetTable,
    settings: QualitySettings,
    source: QualitySource,
    detected: Option<QualityPreset>,
    /// Calibrate before the next game frame.
    bench_due: bool,
    /// The calibration is a "Detect again": it keeps the frame cap and is
    /// stored like a choice.
    redetect: bool,
    bench: Option<Bench>,
    /// Write the settings once known (the first launch's pick).
    save: bool,
    started: bool,
    /// The last game frame's and the last bench frame's animation-frame
    /// time (ms), and when the frame cap lets the next game frame run.
    last_frame: Option<f64>,
    last_bench: Option<f64>,
    next_due: f64,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// This page runs in a phone's or a tablet's browser.
fn mobile() -> bool {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return false;
    };
    let hints = js_sys::Reflect::get(&nav, &"userAgentData".into())
        .ok()
        .filter(|v| v.is_object())
        .and_then(|d| js_sys::Reflect::get(&d, &"mobile".into()).ok())
        .and_then(|m| m.as_bool());
    crate::device::is_mobile(
        hints,
        &nav.user_agent().unwrap_or_default(),
        &nav.platform().unwrap_or_default(),
        nav.max_touch_points().max(0) as u32,
    )
}

impl WebQuality {
    /// `forced` is `?quality=`; `keep` reads and stores the player's
    /// choice under `save_name`.
    pub fn new(save_name: &str, forced: Option<String>, keep: bool, msaa_off: bool) -> Self {
        WebQuality {
            key: format!("trino-quality:{save_name}"),
            keep,
            forced,
            machine: if mobile() {
                Machine::Mobile
            } else {
                Machine::Desktop
            },
            msaa_off,
            table: PresetTable::BUILT_IN,
            settings: QualitySettings::HIGH,
            source: QualitySource::Default,
            detected: None,
            bench_due: false,
            redetect: false,
            bench: None,
            save: false,
            started: false,
            last_frame: None,
            last_bench: None,
            next_due: 0.0,
        }
    }

    /// After the first pack loaded: its presets (`quality/presets`), the
    /// start, applied and published (a calibration waits for the first
    /// animation frame).
    pub fn begin(&mut self, renderer: &mut PcRenderer, platform: &mut WebPlatform) {
        if self.started {
            return;
        }
        self.started = true;
        let blob =
            trino_core::Platform::blob(platform, trino_core::BlobId::from_path("quality/presets"));
        let (table, default) = quality::baked(blob);
        self.table = table;
        platform.quality.set_presets(table);
        let saved = if self.keep {
            storage().and_then(|s| s.get_item(&self.key).ok().flatten())
        } else {
            None
        };
        let start = quality::start(
            self.forced.as_deref(),
            saved.as_deref(),
            self.machine,
            &default,
            &table,
        );
        self.settings = start.settings;
        self.source = start.source;
        self.bench_due = start.bench;
        self.save = start.save && self.keep;
        self.apply(renderer);
        self.publish(renderer, platform);
        if !self.bench_due {
            self.log_line(renderer);
        }
    }

    /// One animation frame at `now` (ms): a calibration's frame, or the
    /// frame cap's skip (false: the game doesn't run this frame), or the
    /// game frame's interval for dynamic resolution (true).
    pub fn tick(
        &mut self,
        renderer: &mut PcRenderer,
        platform: &mut WebPlatform,
        now: f64,
    ) -> bool {
        if self.bench_due {
            self.bench_due = false;
            self.last_bench = None;
            self.bench =
                Some(renderer.bench_begin(&self.table, BENCH_TARGET_US, BENCH_BUDGET, true));
        }
        if let Some(bench) = &mut self.bench {
            let interval = self.last_bench.map(|t| ((now - t) * 1000.0) as u32);
            self.last_bench = Some(now);
            if let Some(cal) = renderer.bench_step(bench, interval) {
                self.bench = None;
                crate::js::log(&format!(
                    "TRINO_QUALITY: bench {} adapter={}",
                    cal.summary(),
                    renderer.adapter_info()
                ));
                let limit = self.settings.frame_limit;
                self.settings = self.table.get(cal.pick);
                if std::mem::take(&mut self.redetect) {
                    self.settings.frame_limit = limit;
                    self.save = true;
                }
                self.detected = Some(cal.pick);
                self.source = QualitySource::Detected;
                self.apply(renderer);
                if self.save {
                    self.persist();
                }
                self.publish(renderer, platform);
                self.log_line(renderer);
                self.last_frame = None;
            }
            return false;
        }
        if let FrameLimit::Fps(n) = self.settings.frame_limit {
            let interval = 1000.0 / n.max(1) as f64;
            if self.last_frame.is_some() && now < self.next_due - 2.0 {
                return false;
            }
            self.next_due = if now - self.next_due > interval {
                now + interval
            } else {
                self.next_due + interval
            };
        }
        if let Some(t) = self.last_frame {
            renderer.report_frame_time(((now - t) * 1000.0).clamp(0.0, u32::MAX as f64) as u32);
        }
        self.last_frame = Some(now);
        true
    }

    /// After the game's frame: its requests, and what it sees next.
    pub fn end_frame(&mut self, renderer: &mut PcRenderer, platform: &mut WebPlatform) {
        match platform.quality.take_request() {
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
                self.redetect = true;
                self.bench_due = true;
            }
            None => {}
        }
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

    /// Put the quality on `meter`'s `TRINO_FRAME` line.
    pub fn report(&self, renderer: &PcRenderer, meter: &mut FrameMeter) {
        meter.set_quality(self.note(renderer));
    }

    fn apply(&mut self, renderer: &mut PcRenderer) {
        let mut q = self.settings;
        if self.msaa_off {
            q.msaa = 1;
        }
        renderer.apply_quality(&q);
    }

    fn persist(&self) {
        if !self.keep {
            return;
        }
        if let Some(s) = storage() {
            let _ = s.set_item(&self.key, &quality::to_text(&self.settings));
        }
    }

    fn publish(&self, renderer: &PcRenderer, platform: &mut WebPlatform) {
        platform.quality.publish(Some(QualityInfo {
            settings: self.settings,
            scale: renderer.render_scale(),
            detected: self.detected,
            source: self.source,
            gpu: renderer.gpu_class(),
            handheld: self.machine != Machine::Desktop,
            gpu_us: renderer.gpu_frame_us(),
        }));
    }

    fn log_line(&self, renderer: &PcRenderer) {
        let line = format!(
            "preset={} source={} scale={:.2} gpu={} mobile={}",
            self.settings.preset.key(),
            self.source.key(),
            renderer.render_scale(),
            renderer.gpu_class().key(),
            self.machine == Machine::Mobile
        );
        crate::js::log(&format!("TRINO_QUALITY: {line}"));
        crate::js::trino_info("quality", &line);
    }
}
