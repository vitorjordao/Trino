//! The ready-made "Graphics" options page (`trino_core::options`, issue
//! #55): it drives a host's quality through the `Platform` calls only, so a
//! fake host that applies requests the way the PC and the Web do stands in
//! for both; a console's fixed quality offers nothing to change.

use trino_core::render::{FontId, SpriteId, SpriteParams, TextAlign, TextParams};
use trino_core::{
    AoSetting, Button, Caps, Color, FrameLimit, GpuClass, GraphicsOptions, InputState, Material,
    ModelId, ModelParams, OptionsEvent, OptionsRow, OptionsStyle, Platform, PresetTable,
    QualityInfo, QualityPreset, QualitySettings, QualitySource, Rect, Renderer, Transform3, Vec2,
};

/// A host that applies the game's requests after the frame, as the PC's
/// and the Web's do.
struct Host {
    table: PresetTable,
    settings: QualitySettings,
    detected: Option<QualityPreset>,
    asked: Option<QualitySettings>,
    detects: u32,
}

impl Host {
    fn new() -> Host {
        Host {
            table: PresetTable::parse("[low]\nrender_scale=0.6\n"),
            settings: QualitySettings::MEDIUM,
            detected: Some(QualityPreset::Medium),
            asked: None,
            detects: 0,
        }
    }

    /// The end of the frame: the request applied (its preset classified).
    fn end_frame(&mut self) {
        if let Some(q) = self.asked.take() {
            self.settings = QualitySettings {
                preset: self.table.classify(&q),
                ..q
            };
        }
    }
}

impl Platform for Host {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn quality(&self) -> QualitySettings {
        self.settings
    }
    fn set_quality(&mut self, settings: QualitySettings) {
        self.asked = Some(settings.clamped());
    }
    fn quality_preset(&self, preset: QualityPreset) -> Option<QualitySettings> {
        (preset != QualityPreset::Custom).then(|| self.table.get(preset))
    }
    fn quality_info(&self) -> Option<QualityInfo> {
        Some(QualityInfo {
            settings: self.settings,
            scale: 0.82,
            detected: self.detected,
            source: QualitySource::Detected,
            gpu: GpuClass::Integrated,
            handheld: false,
            gpu_us: 6_100,
        })
    }
    fn detect_quality(&mut self) {
        self.detects += 1;
    }
}

/// A console: the defaults (fixed Low, nothing hosted).
struct Console;

impl Platform for Console {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
}

/// Press `b` for one frame of the page.
fn press(page: &mut GraphicsOptions, host: &mut dyn Platform, b: Button) -> OptionsEvent {
    let prev = InputState::default();
    let mut now = InputState::default();
    now.set(b, true);
    page.update(host, &now, &prev)
}

fn frame(page: &mut GraphicsOptions, host: &mut Host, b: Button) -> OptionsEvent {
    let e = press(page, host, b);
    host.end_frame();
    // The next frame reads what the host applied.
    page.update(host, &InputState::default(), &InputState::default());
    e
}

#[test]
fn the_page_changes_the_hosts_quality_row_by_row() {
    let mut host = Host::new();
    let mut page = GraphicsOptions::new();
    assert_eq!(press(&mut page, &mut host, Button::A), OptionsEvent::None);
    assert!(!page.is_open(), "closed until the game opens it");
    page.open();
    page.update(&mut host, &InputState::default(), &InputState::default());
    assert_eq!(page.rows(), &OptionsRow::HOSTED);
    assert_eq!(page.row(), OptionsRow::Preset);
    assert_eq!(page.value(OptionsRow::Preset).as_str(), "Medium");

    // The preset: left to the project's Low (its own render scale), the
    // player's frame cap kept.
    host.settings.frame_limit = FrameLimit::Fps(60);
    page.update(&mut host, &InputState::default(), &InputState::default());
    assert_eq!(
        frame(&mut page, &mut host, Button::DpadLeft),
        OptionsEvent::Changed
    );
    assert_eq!(host.settings.preset, QualityPreset::Low);
    assert_eq!(host.settings.render_scale, 0.6);
    assert_eq!(host.settings.frame_limit, FrameLimit::Fps(60));
    // Right twice wraps through Medium to High; left from Low wraps to Ultra.
    frame(&mut page, &mut host, Button::DpadRight);
    frame(&mut page, &mut host, Button::A);
    assert_eq!(host.settings.preset, QualityPreset::High);
    assert_eq!(page.value(OptionsRow::Preset).as_str(), "High");

    // The render scale: 5% steps, Custom (the host classifies it).
    frame(&mut page, &mut host, Button::DpadDown);
    assert_eq!(page.row(), OptionsRow::RenderScale);
    frame(&mut page, &mut host, Button::DpadLeft);
    assert_eq!(host.settings.render_scale, 0.95);
    assert_eq!(host.settings.preset, QualityPreset::Custom);
    assert_eq!(page.value(OptionsRow::RenderScale).as_str(), "95%");
    assert_eq!(page.value(OptionsRow::Preset).as_str(), "Custom");
    // Back up to 1: High again.
    frame(&mut page, &mut host, Button::DpadRight);
    assert_eq!(host.settings.preset, QualityPreset::High);

    // Dynamic resolution: on with the preset's range (the default's for
    // High, which has none), then off.
    frame(&mut page, &mut host, Button::DpadDown);
    frame(&mut page, &mut host, Button::A);
    let d = host.settings.dynamic_resolution.expect("on");
    assert_eq!((d.min_scale, d.max_scale), (0.5, 1.0));
    assert_eq!(
        page.value(OptionsRow::DynamicResolution).as_str(),
        "On 50-100%"
    );
    frame(&mut page, &mut host, Button::A);
    assert_eq!(host.settings.dynamic_resolution, None);
    assert_eq!(page.value(OptionsRow::DynamicResolution).as_str(), "Off");

    // The frame cap cycles its choices; MSAA toggles; AO cycles.
    frame(&mut page, &mut host, Button::DpadDown);
    assert_eq!(page.value(OptionsRow::FrameLimit).as_str(), "60 fps");
    frame(&mut page, &mut host, Button::DpadRight);
    assert_eq!(host.settings.frame_limit, FrameLimit::Fps(120));
    frame(&mut page, &mut host, Button::DpadLeft);
    frame(&mut page, &mut host, Button::DpadLeft);
    assert_eq!(page.value(OptionsRow::FrameLimit).as_str(), "40 fps");
    frame(&mut page, &mut host, Button::DpadDown);
    assert_eq!(page.value(OptionsRow::Msaa).as_str(), "4x MSAA");
    frame(&mut page, &mut host, Button::A);
    assert_eq!(host.settings.msaa, 1);
    assert_eq!(page.value(OptionsRow::Msaa).as_str(), "Off");
    frame(&mut page, &mut host, Button::DpadDown);
    frame(&mut page, &mut host, Button::DpadRight);
    assert_eq!(host.settings.ao, AoSetting::Off, "High wraps to Off");

    // Detect again asks the host; Back (or B) closes.
    frame(&mut page, &mut host, Button::DpadDown);
    assert_eq!(page.row(), OptionsRow::Detect);
    assert_eq!(
        frame(&mut page, &mut host, Button::DpadLeft),
        OptionsEvent::None
    );
    assert_eq!(frame(&mut page, &mut host, Button::A), OptionsEvent::Detect);
    assert_eq!(host.detects, 1);
    frame(&mut page, &mut host, Button::DpadDown);
    assert_eq!(frame(&mut page, &mut host, Button::A), OptionsEvent::Closed);
    assert!(!page.is_open());
    page.open();
    assert_eq!(frame(&mut page, &mut host, Button::B), OptionsEvent::Closed);
    // Up from the first row wraps to the last.
    page.open();
    frame(&mut page, &mut host, Button::DpadUp);
    assert_eq!(page.row(), OptionsRow::Back);
}

#[test]
fn the_page_shows_the_live_scale_and_the_calibrations_pick() {
    let mut host = Host::new();
    let mut page = GraphicsOptions::new();
    page.open();
    page.update(&mut host, &InputState::default(), &InputState::default());
    let (live, detected) = page.info_lines().unwrap();
    assert_eq!(live.as_str(), "3D 82%  GPU 6.1 ms");
    assert_eq!(detected.as_str(), "Detected: Medium (integrated GPU)");
    host.detected = None;
    page.update(&mut host, &InputState::default(), &InputState::default());
    assert_eq!(
        page.info_lines().unwrap().1.as_str(),
        "Detect: not run (integrated GPU)"
    );
}

#[test]
fn a_consoles_fixed_quality_offers_nothing_to_change() {
    let mut console = Console;
    let mut page = GraphicsOptions::new();
    page.open();
    page.update(&mut console, &InputState::default(), &InputState::default());
    assert_eq!(page.rows(), &OptionsRow::FIXED);
    assert_eq!(page.value(OptionsRow::Preset).as_str(), "Low (fixed)");
    assert!(page.info_lines().is_none());
    assert_eq!(
        press(&mut page, &mut console, Button::DpadRight),
        OptionsEvent::None
    );
    assert_eq!(
        press(&mut page, &mut console, Button::DpadDown),
        OptionsEvent::None
    );
    assert_eq!(page.row(), OptionsRow::Back);
    assert_eq!(
        press(&mut page, &mut console, Button::A),
        OptionsEvent::Closed
    );
}

/// What the page draws.
struct Texts {
    caps: Caps,
    rects: Vec<(Rect, Color)>,
    texts: Vec<(String, Vec2, TextAlign, Color)>,
}

impl Renderer for Texts {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: Color) {}
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.rects.push((rect, color));
    }
    fn draw_text(&mut self, _: FontId, pos: Vec2, text: &str, p: &TextParams) {
        self.texts.push((text.to_string(), pos, p.align, p.tint));
    }
}

#[test]
fn the_page_draws_its_rows_values_and_info_in_a_panel() {
    let mut host = Host::new();
    let mut page = GraphicsOptions::new();
    let mut r = Texts {
        caps: Caps::PC,
        rects: Vec::new(),
        texts: Vec::new(),
    };
    let style = OptionsStyle::DEFAULT;
    page.draw(&mut r, FontId(1), Vec2::new(40.0, 30.0), &style);
    assert!(r.texts.is_empty() && r.rects.is_empty(), "closed: nothing");
    page.open();
    page.update(&mut host, &InputState::default(), &InputState::default());
    page.draw(&mut r, FontId(1), Vec2::new(40.0, 30.0), &style);
    let size = page.size(&style);
    // The title, 8 rows and 2 info lines, one line of padding.
    assert_eq!(size, Vec2::new(260.0, 12.0 * 12.0));
    assert_eq!(
        r.rects[0],
        (Rect::new(40.0, 30.0, size.x, size.y), style.panel)
    );
    let text = |s: &str| r.texts.iter().find(|t| t.0 == s).cloned().unwrap();
    assert_eq!(text("Graphics").1, Vec2::new(48.0, 36.0));
    // The picked row is highlighted, its value right-aligned in the accent.
    let quality = text("Quality");
    assert_eq!(quality.3, style.accent);
    let medium = text("Medium");
    assert_eq!(
        (medium.1.x, medium.2, medium.3),
        (292.0, TextAlign::Right, style.accent)
    );
    assert_eq!(medium.1.y, quality.1.y);
    assert_eq!(text("Render scale").3, style.text);
    assert_eq!(text("100%").3, style.dim);
    assert!(
        r.texts
            .iter()
            .any(|t| t.0 == "Detected: Medium (integrated GPU)")
    );
    assert_eq!(r.rects.len(), 2, "the panel and the picked row's bar");
}
