//! PC platform services ([`trino_core::Platform`]): clock, stdout log,
//! baked blobs and a save file.
//!
//! The save lives in one file written atomically (temp file + rename) and
//! is capped at the simulated console's [`Caps::save_bytes`] — an N64
//! profile game that saves 3 KB fails on PC exactly like it would on the
//! cartridge EEPROM.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use trino_core::{
    BindDevice, BlobId, Button, Caps, DisplayInfo, DisplaySettings, GroupId, GroupTable, InputInfo,
    LoadState, MemoryReport, Platform, PresetTable, QualityInfo, QualityPreset, QualitySettings,
    SaveError,
};

use crate::input::{BINDABLE, BINDINGS, InputRequest, PcInput};
use crate::quality::{HostedQuality, QualityRequest};

pub struct PcPlatform {
    start: Instant,
    /// Leaked on insert: blobs are `'static` (see `Platform::blob`). A live
    /// reload leaks the previous version — dev-only, bounded by edits.
    blobs: HashMap<u32, &'static [u8]>,
    save_path: Option<PathBuf>,
    save_capacity: usize,
    /// What the host reports ([`Platform::display`]); `None` = fixed screen.
    display: Option<DisplayInfo>,
    /// The game's last [`Platform::set_display`], for the host to apply.
    display_request: Option<DisplaySettings>,
    /// The simulated second screen (3DS profile).
    second_screen: Option<(u16, u16)>,
    /// What the host reports about input ([`Platform::input_info`]); `None`
    /// until a host publishes it (no remapping then).
    input: Option<InputInfo>,
    pad_name: Option<String>,
    /// Binding labels, [`BINDABLE`] order: keyboard, gamepad.
    labels: [[Option<String>; 2]; BINDINGS],
    /// The game's last rebind/cancel/reset, for the host to apply.
    input_request: Option<InputRequest>,
    /// The simulated console's hardware tier ([`Platform::hw_tier`]).
    hw_tier: trino_core::HwTier,
    /// The quality ([`Platform::quality`] and its siblings): the console's
    /// fixed preset on a simulated console, High on the PC until a host
    /// publishes its own.
    quality: HostedQuality,
    /// The asset groups the bake holds and where each one is (issue #56):
    /// the game asks here, the host's loader (`trino_runner_pc::GroupLoader`)
    /// serves it every frame.
    groups: GroupTable,
    /// Blobs of loaded non-boot groups, freed with their group
    /// ([`Platform::group_blob`]).
    group_blobs: HashMap<u32, Vec<u8>>,
    /// Resident assets by category, kept by the host as it uploads and
    /// frees them ([`Platform::memory`]).
    memory: MemoryReport,
    /// The host's counting allocator, when it installed one.
    heap: Option<&'static dyn trino_core::mem::HeapStats>,
}

impl PcPlatform {
    /// No save file (saves report `Unsupported`) until
    /// [`with_save_file`](Self::with_save_file).
    pub fn new(caps: &Caps) -> Self {
        PcPlatform {
            start: Instant::now(),
            blobs: HashMap::new(),
            save_path: None,
            save_capacity: caps.save_bytes as usize,
            display: None,
            display_request: None,
            second_screen: None,
            input: None,
            pad_name: None,
            labels: Default::default(),
            input_request: None,
            hw_tier: caps.hw_tier,
            quality: HostedQuality::new(caps),
            groups: GroupTable::new(),
            group_blobs: HashMap::new(),
            memory: MemoryReport {
                ram_bytes: caps.ram_bytes,
                ..MemoryReport::default()
            },
            heap: None,
        }
    }

    /// The groups and their states, for the host's loader.
    pub fn groups_mut(&mut self) -> &mut GroupTable {
        &mut self.groups
    }

    /// A non-boot group's blob arrived (its group is loading).
    pub fn insert_group_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        self.group_blobs.insert(id.0, bytes);
    }

    /// Free a group blob; returns its bytes.
    pub fn remove_group_blob(&mut self, id: BlobId) -> usize {
        self.group_blobs.remove(&id.0).map_or(0, |b| b.len())
    }

    /// The resident-asset counts the host keeps ([`Platform::memory`]).
    pub fn memory_mut(&mut self) -> &mut MemoryReport {
        &mut self.memory
    }

    /// Report `heap`'s use as the heap (`#[global_allocator] static A:
    /// trino_core::mem::Counting<std::alloc::System>`).
    pub fn set_heap(&mut self, heap: &'static dyn trino_core::mem::HeapStats) {
        self.heap = Some(heap);
    }

    /// The host simulates a console's tier (`TRINO_SIM=n64x|3dsnew`, the
    /// editor's Game view): what [`Platform::hw_tier`] answers.
    pub fn set_hw_tier(&mut self, tier: trino_core::HwTier) {
        self.hw_tier = tier;
    }

    /// The host publishes the input backend's state: support, a pending
    /// rebind, the pad's name and every binding's label.
    pub fn publish_input(&mut self, input: &PcInput) {
        self.input = Some(input.info());
        self.pad_name = input.gamepad_name();
        for (i, b) in BINDABLE.iter().enumerate() {
            self.labels[i] = [
                input.bindings().label(*b, BindDevice::Keyboard),
                input.bindings().label(*b, BindDevice::Gamepad),
            ];
        }
    }

    /// The rebind, cancel or reset the game asked for since the last call.
    pub fn take_input_request(&mut self) -> Option<InputRequest> {
        self.input_request.take()
    }

    /// The host says the renderer has a second screen (3DS profile).
    pub fn set_second_screen(&mut self, size: Option<(u16, u16)>) {
        self.second_screen = size;
    }

    /// The host publishes the quality in effect and how it got there
    /// (`None`: fixed, the game's requests are dropped).
    pub fn set_quality_info(&mut self, info: Option<QualityInfo>) {
        self.quality.publish(info);
    }

    /// The project's presets (`assets/quality.ron`, baked to
    /// `quality/presets`).
    pub fn set_quality_presets(&mut self, table: PresetTable) {
        self.quality.set_presets(table);
    }

    /// The game's last quality request, for the host to apply after the
    /// frame.
    pub fn take_quality_request(&mut self) -> Option<QualityRequest> {
        self.quality.take_request()
    }

    /// The host publishes the display state (window, monitor, picture).
    pub fn set_display_info(&mut self, info: Option<DisplayInfo>) {
        self.display = info;
    }

    /// The settings the game asked for since the last call, if any.
    pub fn take_display_request(&mut self) -> Option<DisplaySettings> {
        self.display_request.take()
    }

    /// Persist saves at `path` (created on first write).
    pub fn with_save_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.save_path = Some(path.into());
        self
    }

    /// Register (or replace — live reload) a blob.
    pub fn insert_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        self.blobs.insert(id.0, Box::leak(bytes.into_boxed_slice()));
    }
}

impl Platform for PcPlatform {
    fn now_micros(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn load_group(&mut self, group: GroupId) -> LoadState {
        self.groups.request_load(group)
    }

    fn unload_group(&mut self, group: GroupId) {
        self.groups.request_unload(group);
    }

    fn group_state(&self, group: GroupId) -> LoadState {
        self.groups.state(group)
    }

    fn group_blob(&self, id: BlobId) -> Option<&[u8]> {
        match self.group_blobs.get(&id.0) {
            Some(b) => Some(b),
            None => self.blob(id),
        }
    }

    fn memory(&self) -> MemoryReport {
        let clamp = |v: usize| v.min(u32::MAX as usize) as u32;
        let (heap_used, heap_peak) = self
            .heap
            .map_or((0, 0), |h| (clamp(h.used()), clamp(h.peak())));
        MemoryReport {
            heap_used,
            heap_peak,
            groups_resident: self.groups.resident(),
            groups_loading: self.groups.loading(),
            ..self.memory
        }
    }

    fn hw_tier(&self) -> trino_core::HwTier {
        self.hw_tier
    }

    fn log(&self, message: &str) {
        println!("{message}");
    }

    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        self.blobs.get(&id.0).copied()
    }

    fn save_capacity(&self) -> usize {
        if self.save_path.is_some() {
            self.save_capacity
        } else {
            0
        }
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let path = self.save_path.as_ref().ok_or(SaveError::Unsupported)?;
        match std::fs::read(path) {
            Ok(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                Ok(bytes.len())
            }
            // A crash between the temp file and the rename: the temp file
            // is complete (it was flushed before the rename).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::read(path.with_extension("tmp")) {
                    Ok(bytes) => {
                        let n = bytes.len().min(buf.len());
                        buf[..n].copy_from_slice(&bytes[..n]);
                        Ok(bytes.len())
                    }
                    Err(_) => Ok(0),
                }
            }
            Err(_) => Err(SaveError::Io),
        }
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        let path = self.save_path.as_ref().ok_or(SaveError::Unsupported)?;
        if data.len() > self.save_capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity: self.save_capacity,
            });
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| SaveError::Io)?;
        }
        // Write + flush a temp file, then rename over the save: a crash
        // leaves either the old file or the new one, never half of one.
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp).map_err(|_| SaveError::Io)?;
            f.write_all(data).map_err(|_| SaveError::Io)?;
            f.sync_all().map_err(|_| SaveError::Io)?;
        }
        std::fs::rename(&tmp, path).map_err(|_| SaveError::Io)
    }

    fn display(&self) -> Option<DisplayInfo> {
        self.display
    }

    fn second_screen(&self) -> Option<(u16, u16)> {
        self.second_screen
    }

    fn set_display(&mut self, settings: DisplaySettings) {
        if self.display.is_some() {
            self.display_request = Some(settings);
        }
    }

    fn quality(&self) -> QualitySettings {
        self.quality.settings()
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.quality.request(settings);
    }

    fn quality_preset(&self, preset: QualityPreset) -> Option<QualitySettings> {
        self.quality.preset(preset)
    }

    fn quality_info(&self) -> Option<QualityInfo> {
        self.quality.info()
    }

    fn detect_quality(&mut self) {
        self.quality.detect();
    }

    fn input_info(&self) -> Option<InputInfo> {
        self.input
    }

    fn gamepad_name(&self) -> Option<&str> {
        self.pad_name.as_deref()
    }

    fn binding_label(&self, button: Button, device: BindDevice) -> Option<&str> {
        let i = BINDABLE.iter().position(|b| *b == button)?;
        self.labels[i][device as usize].as_deref()
    }

    fn rebind(&mut self, button: Button, device: BindDevice) {
        if self.input.is_some() {
            self.input_request = Some(InputRequest::Rebind(button, device));
        }
    }

    fn cancel_rebind(&mut self) {
        if self.input.is_some() {
            self.input_request = Some(InputRequest::Cancel);
        }
    }

    fn reset_bindings(&mut self) {
        if self.input.is_some() {
            self.input_request = Some(InputRequest::Reset);
        }
    }
}

/// Display settings as a small text file (`key=value` lines), so players
/// can read and fix it by hand.
pub fn display_settings_to_text(s: &DisplaySettings) -> String {
    format!(
        "fullscreen={}\nwindow={}x{}\npixel_scale={}\nui_scale={}\n",
        s.fullscreen as u8, s.window_w, s.window_h, s.pixel_scale, s.ui_scale
    )
}

/// Parse [`display_settings_to_text`]; unknown or broken lines keep the
/// default.
pub fn display_settings_from_text(text: &str) -> DisplaySettings {
    let mut s = DisplaySettings::DEFAULT;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "fullscreen" => s.fullscreen = v == "1" || v == "true",
            "window" => {
                if let Some((w, h)) = v.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    s.window_w = w;
                    s.window_h = h;
                }
            }
            "pixel_scale" => s.pixel_scale = v.parse().unwrap_or(0).min(16),
            "ui_scale" => s.ui_scale = v.parse().unwrap_or(0).min(8),
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_by_handle() {
        let mut p = PcPlatform::new(&Caps::N64);
        let id = BlobId::from_path("blobs/level");
        assert!(p.blob(id).is_none());
        p.insert_blob(id, b"abc".to_vec());
        assert_eq!(p.blob(id), Some(&b"abc"[..]));
    }

    #[test]
    fn display_settings_round_trip_and_requests() {
        let s = DisplaySettings {
            fullscreen: false,
            window_w: 1600,
            window_h: 900,
            pixel_scale: 2,
            ui_scale: 3,
        };
        assert_eq!(display_settings_from_text(&display_settings_to_text(&s)), s);
        assert_eq!(
            display_settings_from_text("garbage\nwindow=axb\n"),
            DisplaySettings::DEFAULT
        );
        // Requests only exist where the host reports a display.
        let mut p = PcPlatform::new(&Caps::PC);
        p.set_display(s);
        assert_eq!(p.take_display_request(), None);
        p.set_display_info(Some(DisplayInfo {
            settings: DisplaySettings::DEFAULT,
            output: (1920, 1080),
            monitor: (1920, 1080),
            screen: (640, 360),
            pixel_scale: 3,
        }));
        p.set_display(s);
        assert_eq!(p.take_display_request(), Some(s));
        assert_eq!(p.take_display_request(), None);
    }

    #[test]
    fn quality_is_fixed_until_a_host_publishes_it() {
        use trino_core::{GpuClass, QualitySource};
        // A simulated console: its fixed preset, requests dropped.
        let mut n64 = PcPlatform::new(&Caps::N64);
        assert_eq!(n64.quality(), QualitySettings::fixed(QualityPreset::Low));
        let n3ds = PcPlatform::new(&Caps::N3DS_NEW);
        assert_eq!(n3ds.quality().preset, QualityPreset::Medium);
        n64.set_quality(QualitySettings::ULTRA);
        n64.detect_quality();
        assert_eq!(n64.take_quality_request(), None);
        assert_eq!(n64.quality_preset(QualityPreset::Low), None);
        assert_eq!(n64.quality_info(), None);
        // The PC: High until its host publishes, then requests go through.
        let mut pc = PcPlatform::new(&Caps::PC);
        assert_eq!(pc.quality(), QualitySettings::HIGH);
        let table = PresetTable::parse("[low]\nrender_scale=0.6\n");
        pc.set_quality_presets(table);
        pc.set_quality_info(Some(QualityInfo {
            settings: QualitySettings::MEDIUM,
            scale: 0.9,
            detected: Some(QualityPreset::Medium),
            source: QualitySource::Detected,
            gpu: GpuClass::Integrated,
            handheld: false,
            gpu_us: 12_000,
        }));
        assert_eq!(pc.quality(), QualitySettings::MEDIUM);
        assert_eq!(pc.quality_info().unwrap().scale, 0.9);
        assert_eq!(
            pc.quality_preset(QualityPreset::Low).unwrap().render_scale,
            0.6
        );
        assert_eq!(pc.quality_preset(QualityPreset::Custom), None);
        pc.set_quality(QualitySettings {
            render_scale: 7.0,
            ..QualitySettings::LOW
        });
        let Some(QualityRequest::Set(q)) = pc.take_quality_request() else {
            panic!("no request");
        };
        assert_eq!(q.render_scale, 2.0, "requests are clamped");
        pc.detect_quality();
        assert_eq!(pc.take_quality_request(), Some(QualityRequest::Detect));
        assert_eq!(pc.take_quality_request(), None);
    }

    #[test]
    fn save_round_trip_and_budget() {
        let dir = std::env::temp_dir().join(format!("trino-save-{}", std::process::id()));
        let path = dir.join("game.sav");
        let mut p = PcPlatform::new(&Caps::N64).with_save_file(&path);
        assert_eq!(p.save_capacity(), 2048);
        let mut buf = [0u8; 16];
        assert_eq!(p.save_read(&mut buf), Ok(0), "no save yet");
        p.save_write(b"hello").unwrap();
        assert_eq!(p.save_read(&mut buf), Ok(5));
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(
            p.save_write(&[0u8; 4096]),
            Err(SaveError::TooLarge {
                len: 4096,
                capacity: 2048
            })
        );
        let mut none = PcPlatform::new(&Caps::N64);
        assert_eq!(none.save_write(b"x"), Err(SaveError::Unsupported));
        assert_eq!(none.save_capacity(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
