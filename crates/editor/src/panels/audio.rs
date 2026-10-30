//! Assets › Audio › the audio inspector (issue #53): what each platform
//! bakes a sound or a music track to. The master's waveform with the
//! chosen platform's decoded result over it and the codec's noise under
//! it; Play buttons for the master and every platform's decode (hear the
//! VADPCM / DSP-ADPCM / Vorbis result on the PC, on the sound's bus); the
//! shared keys (`stream`, `loop`, `bus`, Vorbis `quality`); and one column
//! per platform: its `codec` and `sample_rate` entries, what the bake uses,
//! its bytes, the ratio against PCM16, the SNR against the master (colored
//! against the codec's floor) and what it costs in RAM. Edits write the
//! manifest (the watcher rebakes); `cargo xtask audio inspect` prints the
//! same numbers.

use eframe::egui::{self, Color32, RichText, Vec2};
use trino_asset_pipeline::Platform;
use trino_asset_pipeline::audio::{Codec, DEFAULT_QUALITY};
use trino_core::Bus;

use crate::app::{App, Cmd};
use crate::audio::{
    Column, Inspection, Measured, Wave, codec_name, platform_label, snr_floor, storage_name,
};
use crate::panels::texture::{row, small};
use crate::textures::bytes;
use crate::theme::{self, c, icon};

/// Shown in a combo for "no entry".
const DEFAULT: &str = "default";
/// The rates the rate combos offer (the 3DS DSP's native 32 728 Hz too).
const RATES: [u32; 8] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 32_728, 44_100, 48_000,
];
/// The codec noise strip is drawn this much louder than the waveform.
const NOISE_GAIN: f32 = 8.0;

/// What plays in the preview player: (logical, platform; `None` = master).
pub type Playing = Option<(String, Option<Platform>)>;

/// The sections, under the asset's header.
pub fn section(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let key = crate::audio::key(&app.project.manifest_path(), app.project.generation);
    let assets = app.project.assets_dir();
    let (insp, errors) = app.audio.inspect(ui.ctx(), &assets, logical, key);
    let ready = insp.is_some();
    // A bake error once in the Console (and a toast), always in the panel.
    for e in errors {
        app.log.toast(crate::log::Level::Error, e);
    }
    let measuring = app.audio.measuring();
    // The caption (issue #96): read apart from the codecs, drawn first.
    let keys = app.audio.caption(&assets, logical, key);
    caption_section(app, ui, logical, keys);
    let playing: Playing = app.audio.player.as_mut().and_then(|p| p.playing().cloned());
    if playing.is_some() {
        // Notice when it ends.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    let crate::audio::AudioView {
        current,
        preview,
        quality_edit,
        ..
    } = &mut app.audio;
    let Some((_, insp)) = current.as_ref().filter(|_| ready) else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Measuring codecs…").color(c::TEXT_WEAK));
        });
        return;
    };
    // `TRINO_EDITOR_INSPECT=audio`: scroll the platforms into view.
    let focus = app.inspect_focus.as_deref() == Some("audio");
    let mut cmds = Vec::new();
    summary(ui, insp);
    theme::component(
        ui,
        "audio-waveform",
        icon::WAVEFORM,
        c::ACCENT_TEXT,
        "Waveform",
        |ui| {
            if measuring {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Measuring…").size(11.5).color(c::TEXT_FAINT));
                });
            }
            waveform(ui, insp, preview);
            ui.add_space(4.0);
            play_row(ui, insp, &playing, &mut cmds);
        },
    );
    theme::component(
        ui,
        "audio-settings",
        icon::SLIDERS,
        c::ACCENT_TEXT,
        "Bake settings",
        |ui| settings(ui, insp, quality_edit, &mut cmds),
    );
    if focus {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(ui.cursor().min, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
    theme::component(
        ui,
        "audio-platforms",
        icon::STACK,
        c::ACCENT_TEXT,
        "Platforms",
        |ui| {
            grid(ui, insp, &mut cmds);
            ui.add_space(4.0);
            notes(ui, insp);
        },
    );
    app.cmds.extend(cmds);
}

/// The master: file, rate, length.
fn summary(ui: &mut egui::Ui, insp: &Inspection) {
    crate::app::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(&insp.file).color(c::TEXT)).truncate());
        let text = match &insp.master {
            Ok((rate, pcm)) => format!(
                "{:.2} s · {} Hz · {} frames · mono",
                pcm.len() as f64 / (*rate).max(1) as f64,
                rate,
                pcm.len()
            ),
            Err(e) => e.clone(),
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(c::TEXT_FAINT)).truncate());
    });
    ui.add_space(6.0);
}

/// The master's envelope (gray), the chosen platform's decode over it
/// (accent) and that decode's error ×8 underneath (red).
fn waveform(ui: &mut egui::Ui, insp: &Inspection, preview: &mut usize) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(
            RichText::new("Over the master:")
                .size(11.5)
                .color(c::TEXT_FAINT),
        );
        for (i, col) in insp.columns.iter().enumerate() {
            let r = ui.selectable_label(*preview == i, platform_label(col.platform));
            if r.clicked() {
                *preview = i;
            }
        }
    });
    let i = (*preview).min(insp.columns.len().saturating_sub(1));
    let measured = insp.columns.get(i).and_then(|c| c.result.as_ref().ok());
    let width = ui.available_width().max(60.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 112.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, c::BG0);
    let wave_rect = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - 28.0));
    let noise_rect = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 26.0), rect.max);
    painter.line_segment(
        [
            egui::pos2(wave_rect.min.x, wave_rect.center().y),
            egui::pos2(wave_rect.max.x, wave_rect.center().y),
        ],
        egui::Stroke::new(1.0, c::BORDER),
    );
    paint_wave(
        &painter,
        wave_rect,
        &insp.master_wave,
        c::TEXT_FAINT,
        1.0,
        false,
    );
    let label = match measured {
        Some(m) => {
            if let Some(w) = &m.wave {
                paint_wave(
                    &painter,
                    wave_rect,
                    w,
                    c::ACCENT.gamma_multiply(0.75),
                    1.0,
                    false,
                );
            }
            if let Some(n) = &m.noise {
                paint_wave(&painter, noise_rect, n, c::ERROR, NOISE_GAIN, true);
            }
            format!(
                "{} {} {}",
                platform_label(insp.columns[i].platform),
                codec_name(m.settings.codec),
                if m.decoded.is_none() {
                    "(no host decode)"
                } else {
                    ""
                }
            )
        }
        None => "bake fails".into(),
    };
    let font = egui::FontId::proportional(11.0);
    painter.text(
        wave_rect.left_top() + Vec2::new(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        "master",
        font.clone(),
        c::TEXT_WEAK,
    );
    painter.text(
        wave_rect.left_top() + Vec2::new(6.0, 18.0),
        egui::Align2::LEFT_TOP,
        label.trim_end(),
        font.clone(),
        c::ACCENT_TEXT,
    );
    painter.text(
        noise_rect.left_center() + Vec2::new(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        format!("codec noise ×{NOISE_GAIN:.0}"),
        font.clone(),
        c::ERROR.gamma_multiply(0.9),
    );
    if let Ok((rate, pcm)) = &insp.master {
        painter.text(
            wave_rect.right_top() + Vec2::new(-6.0, 4.0),
            egui::Align2::RIGHT_TOP,
            format!("{:.2} s", pcm.len() as f64 / (*rate).max(1) as f64),
            font,
            c::TEXT_FAINT,
        );
    }
}

/// `wave` as one vertical line per pixel column: centered (a waveform),
/// or from the bottom (`from_bottom`: an error envelope), × `gain`.
fn paint_wave(
    painter: &egui::Painter,
    rect: egui::Rect,
    wave: &Wave,
    color: Color32,
    gain: f32,
    from_bottom: bool,
) {
    let n = wave.max.len();
    if n == 0 {
        return;
    }
    let cols = rect.width().floor().max(1.0) as usize;
    let half = rect.height() * 0.5 - 2.0;
    for x in 0..cols {
        let (b0, b1) = (
            x * n / cols,
            ((x + 1) * n / cols).max(x * n / cols + 1).min(n),
        );
        let lo = wave.min[b0..b1].iter().copied().fold(0.0f32, f32::min);
        let hi = wave.max[b0..b1].iter().copied().fold(0.0f32, f32::max);
        let px = rect.min.x + x as f32 + 0.5;
        let (y0, y1) = if from_bottom {
            let h = (hi * gain).clamp(0.0, 1.0) * (rect.height() - 2.0);
            (rect.max.y - 1.0 - h, rect.max.y - 1.0)
        } else {
            let mid = rect.center().y;
            (
                mid - (hi * gain).clamp(-1.0, 1.0) * half,
                mid - (lo * gain).clamp(-1.0, 1.0) * half,
            )
        };
        if y1 - y0 >= 0.5 {
            painter.line_segment(
                [egui::pos2(px, y0), egui::pos2(px, y1)],
                egui::Stroke::new(1.0, color),
            );
        }
    }
}

/// Play the master or a platform's decode (on the PC's output); Stop.
fn play_row(ui: &mut egui::Ui, insp: &Inspection, playing: &Playing, cmds: &mut Vec<Cmd>) {
    let now = |p: Option<Platform>| {
        playing
            .as_ref()
            .is_some_and(|(l, q)| *l == insp.logical && *q == p)
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let button = |ui: &mut egui::Ui, text: &str, name: &str, on: bool, enabled: bool| {
            let glyph = if on { icon::SPEAKER_HIGH } else { icon::PLAY };
            let mut b = egui::Button::new(RichText::new(format!("{glyph} {text}")).size(12.0))
                .min_size(Vec2::new(0.0, 24.0));
            if on {
                b = b
                    .fill(c::ACCENT_SOFT)
                    .stroke(egui::Stroke::new(1.0, c::ACCENT));
            }
            let r = ui.add_enabled(enabled, b);
            let name = name.to_string();
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
            r
        };
        let master_ok = insp.master.is_ok();
        if button(ui, "Master", "Play master", now(None), master_ok)
            .on_hover_text("The master as authored")
            .clicked()
        {
            cmds.push(Cmd::PlayAudio(insp.logical.clone(), None));
        }
        for col in &insp.columns {
            let p = col.platform;
            let decoded = col.result.as_ref().is_ok_and(|m| m.decoded.is_some());
            let hover = match &col.result {
                Ok(m) if m.decoded.is_some() => format!(
                    "What the {} plays: {} at {} Hz, decoded here",
                    platform_label(p),
                    codec_name(m.settings.codec),
                    m.rate
                ),
                Ok(m) => format!(
                    "{} is decoded by the console only: no preview on the PC",
                    codec_name(m.settings.codec)
                ),
                Err(e) => e.clone(),
            };
            let name = format!("Play {}", platform_label(p));
            if button(ui, platform_label(p), &name, now(Some(p)), decoded)
                .on_hover_text(hover)
                .on_disabled_hover_text("Nothing to play: see the notes below")
                .clicked()
            {
                cmds.push(Cmd::PlayAudio(insp.logical.clone(), Some(p)));
            }
        }
        if playing.is_some() && theme::named_icon_button(ui, icon::STOP, "Stop").clicked() {
            cmds.push(Cmd::StopAudio);
        }
    });
}

/// The keys every platform shares: stream, loop, bus, Vorbis quality.
fn settings(
    ui: &mut egui::Ui,
    insp: &Inspection,
    quality_edit: &mut Option<f32>,
    cmds: &mut Vec<Cmd>,
) {
    let Some(s) = insp.settings().copied() else {
        ui.label(RichText::new("No platform bakes it: see the notes").color(c::ERROR));
        return;
    };
    let l = &insp.logical;
    let music = insp.music;
    // A value equal to the default leaves the manifest (no key).
    let bool_key = |key: &'static str, v: bool, default: bool| {
        Cmd::SetAudioKey(l.clone(), key, (v != default).then(|| v.to_string()))
    };
    theme::prop(ui, "Stream", |ui| {
        let mut v = s.stream;
        let r = ui
            .checkbox(
                &mut v,
                if insp.stream.is_some() {
                    ""
                } else {
                    "(default)"
                },
            )
            .on_hover_text(
                "Play from storage as it goes instead of loading it whole: RAM stays flat \
                 however long it is (music streams by default; the N64 always does)",
            );
        r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, v, "Stream"));
        if r.changed() {
            cmds.push(bool_key("stream", v, music));
        }
    });
    theme::prop(ui, "Loop", |ui| {
        let mut v = s.looped;
        let r = ui
            .checkbox(
                &mut v,
                if insp.looped.is_some() {
                    ""
                } else {
                    "(default)"
                },
            )
            .on_hover_text("Baked loop (on the N64 the loop lives in the wav64)");
        r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, v, "Loop"));
        if r.changed() {
            cmds.push(bool_key("loop", v, music));
        }
    });
    theme::prop(ui, "Bus", |ui| {
        let default = if music { Bus::Music } else { Bus::Sfx };
        let shown = match &insp.bus {
            Some(b) => b.clone(),
            None => format!("{DEFAULT} ({})", default.label()),
        };
        let mut pick: Option<Option<String>> = None;
        let r = egui::ComboBox::from_id_salt("audio-bus")
            .selected_text(shown)
            .width(ui.available_width().min(150.0))
            .truncate()
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        insp.bus.is_none(),
                        format!("{DEFAULT} ({})", default.label()),
                    )
                    .clicked()
                {
                    pick = Some(None);
                }
                for b in Bus::ALL.into_iter().filter(|b| *b != Bus::Master) {
                    if ui
                        .selectable_label(insp.bus.as_deref() == Some(b.label()), b.label())
                        .clicked()
                    {
                        pick = Some(Some(b.label().to_string()));
                    }
                }
            });
        r.response
            .on_hover_text("The mixer bus it plays on unless the game names one")
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Bus"));
        if let Some(b) = pick
            && b != insp.bus
        {
            cmds.push(Cmd::SetAudioKey(l.clone(), "bus", b));
        }
    });
    let vorbis = insp.columns.iter().any(|c| {
        c.result
            .as_ref()
            .is_ok_and(|m| m.settings.codec == Codec::Vorbis)
    });
    theme::prop(ui, "Quality", |ui| {
        let declared = insp.quality;
        // On the slider's steps already: it snaps what it shows, and a
        // snap reads as a change (a hand-written 6.3 would be rewritten).
        let mut v = (quality_edit.or(declared).unwrap_or(DEFAULT_QUALITY) * 2.0).round() / 2.0;
        let r = ui
            .add_enabled(
                vorbis,
                egui::Slider::new(&mut v, 0.0..=10.0)
                    .step_by(0.5)
                    .max_decimals(1),
            )
            .on_hover_text(
                "Vorbis quality (PC, Web), like oggenc -q: higher keeps more of the master \
                 and costs bytes",
            )
            .on_disabled_hover_text("Only Vorbis has a quality: no platform uses it here");
        name_slider(&r, "Vorbis quality");
        if r.changed() {
            *quality_edit = Some(v);
        }
        // Written once the user settles (a drag must not rebake per frame).
        if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
            && let Some(v) = *quality_edit
            && Some(v) != declared
        {
            cmds.push(Cmd::SetAudioKey(l.clone(), "quality", Some(format!("{v}"))));
        }
        if declared.is_some()
            && theme::named_icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Default quality")
                .clicked()
        {
            *quality_edit = None;
            cmds.push(Cmd::SetAudioKey(l.clone(), "quality", None));
        }
    });
}

fn grid(ui: &mut egui::Ui, insp: &Inspection, cmds: &mut Vec<Cmd>) {
    let label_w = 60.0;
    let cols = &insp.columns;
    let n = cols.len().max(1);
    let col_w = ((ui.available_width() - label_w - n as f32 * 4.0) / n as f32).floor();
    let widths = (label_w, col_w, n);
    let master_rate = insp.master.as_ref().ok().map(|(r, _)| *r);
    // A narrow panel can't fit a combo per column: full-width combos per
    // platform above the grid instead.
    let narrow = col_w < 90.0;
    if narrow {
        for col in cols {
            theme::prop(ui, platform_label(col.platform), |ui| {
                let w = ((ui.available_width() - 4.0) / 2.0).min(120.0);
                codec_combo(ui, &insp.logical, col, w, cmds);
                rate_combo(ui, &insp.logical, col, master_rate, w, cmds);
            });
        }
    }
    row(ui, "", widths, |ui, i| {
        ui.label(
            RichText::new(platform_label(cols[i].platform))
                .size(12.5)
                .family(theme::bold())
                .color(c::TEXT),
        );
    });
    if !narrow {
        row(ui, "Codec", widths, |ui, i| {
            codec_combo(ui, &insp.logical, &cols[i], col_w, cmds);
        });
        row(ui, "Rate", widths, |ui, i| {
            rate_combo(ui, &insp.logical, &cols[i], master_rate, col_w, cmds);
        });
    }
    row(ui, "Uses", widths, |ui, i| match &cols[i].result {
        Ok(m) => {
            let text = if m.settings.auto {
                format!("{} · auto", codec_name(m.settings.codec))
            } else {
                codec_name(m.settings.codec).to_string()
            };
            small(
                ui,
                text,
                if m.settings.auto {
                    c::ACCENT_TEXT
                } else {
                    c::TEXT
                },
            )
            .on_hover_text(format!(
                "{} at {} Hz, {} frames{}",
                codec_name(m.settings.codec),
                m.rate,
                m.frames,
                if m.settings.stream { ", streamed" } else { "" }
            ));
        }
        Err(e) => {
            small(ui, format!("{} bake fails", icon::WARNING), c::ERROR).on_hover_text(e);
        }
    });
    row(ui, "Size", widths, |ui, i| {
        if let Ok(m) = &cols[i].result {
            let text = if m.exact {
                bytes(m.bytes as u64)
            } else {
                format!("≈{}", bytes(m.bytes as u64))
            };
            small(ui, text, c::TEXT_WEAK).on_hover_text(format!(
                "{} bytes in the {}{}",
                m.bytes,
                storage_name(cols[i].platform),
                if m.exact {
                    ""
                } else {
                    " (estimated: audioconv64 makes it in the N64 build)"
                }
            ));
        }
    });
    row(ui, "Ratio", widths, |ui, i| {
        if let Ok(m) = &cols[i].result {
            let r = m.ratio();
            small(
                ui,
                format!("{r:.1}×"),
                if r > 1.5 { c::OK } else { c::TEXT_WEAK },
            )
            .on_hover_text(format!(
                "{} as PCM16 at {} Hz",
                bytes(m.pcm16_bytes as u64),
                m.rate
            ));
        }
    });
    row(ui, "SNR", widths, |ui, i| {
        if let Ok(m) = &cols[i].result {
            let (text, color) = snr_cell(m);
            small(ui, text, color).on_hover_text(match (m.snr, snr_floor(m.settings.codec)) {
                (None, _) => "Not decodable on the host: listen on the console".to_string(),
                (Some(_), Some(f)) => format!(
                    "Signal-to-noise against the master; {} should keep {f} dB",
                    codec_name(m.settings.codec)
                ),
                (Some(_), None) => "Signal-to-noise against the master".to_string(),
            });
        }
    });
    row(ui, "Memory", widths, |ui, i| {
        if let Ok(m) = &cols[i].result {
            let (text, hover) = memory_cell(cols[i].platform, m);
            small(ui, text, c::TEXT_WEAK).on_hover_text(hover);
        }
    });
}

/// The SNR cell: green at or over the codec's floor, amber under it.
fn snr_cell(m: &Measured) -> (String, Color32) {
    match m.snr {
        None => ("—".into(), c::TEXT_FAINT),
        Some(s) if s.is_infinite() || s > 90.0 => ("lossless".into(), c::OK),
        Some(s) => (
            format!("{s:.1} dB"),
            match snr_floor(m.settings.codec) {
                Some(f) if s < f => c::WARN,
                _ => c::OK,
            },
        ),
    }
}

/// The memory cell and its hover: what stays in RAM, and what a playback
/// adds.
fn memory_cell(p: Platform, m: &Measured) -> (String, String) {
    let f = m.footprint;
    match p {
        Platform::N64 => (
            "ROM stream".into(),
            format!(
                "The mixer reads the wav64 from ROM as it plays: {} of RDRAM while open, +{} of \
                 mixer buffer per playing channel",
                bytes(f.resident as u64),
                bytes(f.playing as u64)
            ),
        ),
        _ if m.settings.stream => (
            format!("+{}", bytes(f.playing as u64)),
            format!(
                "Streams: {} held, +{} per playback while it plays",
                bytes(f.resident as u64),
                bytes(f.playing as u64)
            ),
        ),
        Platform::N3ds => (
            bytes(f.resident as u64),
            "Loaded whole into linear memory, decoded by the DSP".into(),
        ),
        _ => (
            bytes(f.resident as u64),
            "Decoded once at load to f32 at the device rate (48 kHz)".into(),
        ),
    }
}

/// A platform's `codec` entry: the default (no entry: `auto`), `auto`, or
/// a codec it plays. Its accessible label is "<platform> codec".
fn codec_combo(ui: &mut egui::Ui, logical: &str, col: &Column, width: f32, cmds: &mut Vec<Cmd>) {
    let p = col.platform;
    let shown = col.declared_codec.clone().unwrap_or_else(|| DEFAULT.into());
    let mut pick: Option<Option<String>> = None;
    let r = egui::ComboBox::from_id_salt(("audio-codec", p.key()))
        .selected_text(shown)
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            let auto = codec_name(Codec::auto(p));
            if ui
                .selectable_label(
                    col.declared_codec.is_none(),
                    format!("{DEFAULT} (auto: {auto})"),
                )
                .clicked()
            {
                pick = Some(None);
            }
            for c in std::iter::once("auto").chain(Codec::supported(p).iter().map(|c| c.label())) {
                if ui
                    .selectable_label(col.declared_codec.as_deref() == Some(c), c)
                    .clicked()
                {
                    pick = Some(Some(c.to_string()));
                }
            }
        });
    let name = format!("{} codec", platform_label(p));
    r.response
        .on_hover_text("The manifest's `codec` entry (default removes it)")
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    if let Some(v) = pick
        && v != col.declared_codec
    {
        cmds.push(Cmd::SetAudioPlatform(logical.into(), "codec", p, v));
    }
}

/// A platform's `sample_rate` entry: the master's rate (no entry) or a
/// resample. Its accessible label is "<platform> rate".
fn rate_combo(
    ui: &mut egui::Ui,
    logical: &str,
    col: &Column,
    master: Option<u32>,
    width: f32,
    cmds: &mut Vec<Cmd>,
) {
    let p = col.platform;
    let khz = |r: u32| format!("{:.1} kHz", r as f64 / 1000.0);
    let shown = match col.declared_rate {
        Some(r) => khz(r),
        None => "master".into(),
    };
    let mut pick: Option<Option<u32>> = None;
    let r = egui::ComboBox::from_id_salt(("audio-rate", p.key()))
        .selected_text(shown)
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            let master = master.map_or(String::new(), |r| format!(" ({})", khz(r)));
            if ui
                .selectable_label(col.declared_rate.is_none(), format!("master{master}"))
                .clicked()
            {
                pick = Some(None);
            }
            for r in RATES {
                if ui
                    .selectable_label(col.declared_rate == Some(r), khz(r))
                    .clicked()
                {
                    pick = Some(Some(r));
                }
            }
        });
    let name = format!("{} rate", platform_label(p));
    r.response
        .on_hover_text("The manifest's `sample_rate` entry: resample the master for it")
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    if let Some(v) = pick
        && v != col.declared_rate
    {
        cmds.push(Cmd::SetAudioPlatform(
            logical.into(),
            "sample_rate",
            p,
            v.map(|r| r.to_string()),
        ));
    }
}

/// Name a slider for accessibility. Not with `widget_info`: a Slider's
/// response covers its value box too, and that would stretch the node
/// over the box (the layout audit's overlap).
pub(crate) fn name_slider(r: &egui::Response, name: &str) {
    r.ctx
        .accesskit_node_builder(r.id, |b| b.set_label(name.to_string()));
}

/// A line per platform worth one: bake errors, Opus's estimate, an SNR
/// under the codec's floor.
fn notes(ui: &mut egui::Ui, insp: &Inspection) {
    for col in &insp.columns {
        let p = platform_label(col.platform);
        let (text, color) = match &col.result {
            Err(e) => (format!("{p}: {e}"), c::ERROR),
            Ok(m) if m.decoded.is_none() => (
                format!(
                    "{p}: {} is decoded by the console (CPU + RSP): its size is an estimate \
                     until the N64 build runs audioconv64, and the PC can't play it",
                    codec_name(m.settings.codec)
                ),
                c::TEXT_FAINT,
            ),
            Ok(m) => match (m.snr, snr_floor(m.settings.codec)) {
                (Some(s), Some(f)) if s < f => (
                    format!(
                        "{p}: {} keeps {s:.1} dB, under the {f} dB it should — try a higher \
                         rate or quality, or `pcm`",
                        codec_name(m.settings.codec)
                    ),
                    c::WARN,
                ),
                _ => continue,
            },
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).wrap());
    }
}

/// The platform budget's audio lines (scene settings): the project's audio
/// per platform, measured like the bake on a worker.
pub fn budget(
    ui: &mut egui::Ui,
    view: &mut crate::audio::AudioView,
    project: &crate::project::Project,
) {
    let line = |ui: &mut egui::Ui, text: String, color: Color32| {
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).truncate())
    };
    let key = crate::audio::key(&project.manifest_path(), project.generation);
    let Some(t) = view.totals(ui.ctx(), &project.assets_dir(), key) else {
        // A line per platform already: what follows doesn't move when the
        // numbers arrive (a click there would land elsewhere).
        for p in Platform::ALL {
            line(
                ui,
                format!("Audio {}: measuring…", platform_label(p)),
                c::TEXT_FAINT,
            );
        }
        return;
    };
    for p in Platform::ALL {
        let pt = t.of(p);
        line(
            ui,
            crate::audio::budget_line(pt, p),
            if pt.errors > 0 {
                c::ERROR
            } else {
                c::TEXT_WEAK
            },
        )
        .on_hover_text(crate::audio::budget_hover(pt, p, t.sounds));
    }
}

/// The sound's caption (issue #96): its string-table key (a combo of the
/// tables' keys, or a new one added to every language), the text in each
/// language, `important`, `caption_priority`, `direction` and
/// `caption_seconds`, a Preview in the Game view, and the checklist's lines
/// for this sound (`cargo xtask access check`'s `check_captions`). Every
/// change writes the manifest.
fn caption_section(
    app: &mut App,
    ui: &mut egui::Ui,
    logical: &str,
    keys: Result<crate::audio::CaptionKeys, String>,
) {
    let keys = match keys {
        Ok(k) => k,
        Err(e) => {
            ui.label(RichText::new(e).size(11.5).color(c::ERROR));
            return;
        }
    };
    let l = logical.to_string();
    let mut cmds: Vec<Cmd> = Vec::new();
    let mut new_key: Option<String> = None;
    let mut preview = false;
    let all_keys: Vec<String> = app
        .localization
        .langs
        .keys()
        .into_iter()
        .map(str::to_string)
        .collect();
    let langs = app.localization.langs.names();
    let findings = trino_scene::access::check_captions(
        &[trino_scene::access::CaptionDecl {
            logical: l.clone(),
            caption: keys.key.clone(),
            important: keys.important.unwrap_or(false),
        }],
        &app.localization.langs,
    );
    let editing = &mut app.audio.new_caption_key;
    let place = &mut app.audio.caption_place;
    theme::component(
        ui,
        "audio-caption",
        icon::SUBTITLES,
        c::ACCENT_TEXT,
        "Caption",
        |ui| {
            theme::prop(ui, "Key", |ui| {
                let shown = keys.key.clone().unwrap_or_else(|| "none".into());
                let mut pick: Option<Option<String>> = None;
                let w = ui.available_width().min(200.0);
                theme::fixed_width(ui, w, |ui| {
                    egui::ComboBox::from_id_salt("audio-caption-key")
                        .selected_text(shown)
                        .width(w - 8.0)
                        .truncate()
                        .height(300.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(keys.key.is_none(), "none").clicked() {
                                pick = Some(None);
                            }
                            if ui.selectable_label(false, "New key…").clicked() {
                                *editing = Some(format!(
                                    "cap.{}",
                                    l.rsplit('/').next().unwrap_or_default()
                                ));
                            }
                            for k in &all_keys {
                                if ui
                                    .selectable_label(keys.key.as_deref() == Some(k), k)
                                    .clicked()
                                {
                                    pick = Some(Some(k.clone()));
                                }
                            }
                        })
                        .response
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::ComboBox,
                                true,
                                "Caption key",
                            )
                        });
                });
                if let Some(p) = pick
                    && p != keys.key
                {
                    cmds.push(Cmd::SetAudioKey(l.clone(), "caption", p));
                }
            });
            if let Some(text) = editing {
                ui.horizontal(|ui| {
                    ui.add_space(64.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text("cap.whistle")
                            .desired_width(150.0),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            true,
                            "New caption key",
                        )
                    });
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if enter || ui.button("Add").clicked() {
                        new_key = Some(text.trim().to_string());
                    }
                });
            }
            // The text in each shipped language (edited in the Localization
            // tab), and which language lacks it.
            if let Some(k) = &keys.key {
                if langs.is_empty() {
                    ui.label(
                        RichText::new("No string table: the caption shows its key")
                            .size(11.5)
                            .color(c::WARN),
                    );
                }
                for lang in &langs {
                    let strings = app.localization.langs.strings(lang);
                    let own = strings.has_own(k);
                    let text = trino_core::ui::UiStrings::get(&strings, k).map(str::to_string);
                    ui.horizontal(|ui| {
                        ui.add_space(64.0);
                        ui.label(RichText::new(lang).size(11.5).color(c::TEXT_FAINT));
                        match (own, text) {
                            (true, Some(t)) => {
                                ui.add(
                                    egui::Label::new(RichText::new(t).size(12.0).color(c::TEXT))
                                        .truncate(),
                                );
                            }
                            (false, t) => {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "{} missing in {lang}{}",
                                            icon::WARNING,
                                            t.map_or(String::new(), |t| format!(
                                                " (shows \"{t}\")"
                                            ))
                                        ))
                                        .size(11.5)
                                        .color(c::WARN),
                                    )
                                    .truncate(),
                                );
                            }
                            (true, None) => {}
                        }
                    });
                }
            }
            let bool_row = |ui: &mut egui::Ui,
                            cmds: &mut Vec<Cmd>,
                            name: &'static str,
                            key: &'static str,
                            v: Option<bool>,
                            hint: &str| {
                theme::prop(ui, name, |ui| {
                    let mut on = v.unwrap_or(false);
                    let r = ui.checkbox(&mut on, "").on_hover_text(hint);
                    r.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name)
                    });
                    if r.changed() {
                        cmds.push(Cmd::SetAudioKey(l.clone(), key, on.then(|| "true".into())));
                    }
                });
            };
            bool_row(
                ui,
                &mut cmds,
                "Important",
                "important",
                keys.important,
                "Shown in the \"important\" caption mode too (a whistle, an alarm); needs a caption",
            );
            bool_row(
                ui,
                &mut cmds,
                "Direction",
                "direction",
                keys.direction,
                "An arrow toward the sound's pan",
            );
            theme::prop(ui, "Priority", |ui| {
                let default = if keys.important == Some(true) { 100 } else { 0 };
                let mut v = keys.priority.unwrap_or(default) as u32;
                let r = ui
                    .add(egui::DragValue::new(&mut v).range(0..=255))
                    .on_hover_text("A full caption queue drops the lowest first");
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Caption priority")
                });
                if r.changed() {
                    cmds.push(Cmd::SetAudioKey(
                        l.clone(),
                        "caption_priority",
                        Some(v.to_string()),
                    ));
                }
            });
            theme::prop(ui, "Seconds", |ui| {
                let mut v = keys
                    .seconds
                    .unwrap_or(trino_core::access::captions::DEFAULT_SECONDS);
                let r = ui.add(
                    egui::DragValue::new(&mut v)
                        .range(0.1..=60.0)
                        .speed(0.1)
                        .max_decimals(1)
                        .suffix(" s"),
                );
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Caption seconds")
                });
                if r.changed() {
                    cmds.push(Cmd::SetAudioKey(
                        l.clone(),
                        "caption_seconds",
                        Some(format!("{v}")),
                    ));
                }
            });
            theme::prop(ui, "Preview", |ui| {
                use trino_core::access::CaptionPlace;
                let name = |p: CaptionPlace| match p {
                    CaptionPlace::Bottom => "bottom",
                    CaptionPlace::Top => "top",
                    CaptionPlace::SecondScreen => "3DS touch screen",
                };
                egui::ComboBox::from_id_salt("audio-caption-place")
                    .selected_text(name(*place))
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for p in [
                            CaptionPlace::Bottom,
                            CaptionPlace::Top,
                            CaptionPlace::SecondScreen,
                        ] {
                            ui.selectable_value(place, p, name(p));
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Caption place")
                    });
                if ui
                .add_enabled(keys.key.is_some(), egui::Button::new(format!("{} Preview", icon::PLAY)))
                .on_hover_text(
                    "Show the caption in the Game view at the Accessibility tab's text size and \
                     contrast",
                )
                .clicked()
            {
                preview = true;
            }
            });
            // The checklist's lines for this sound.
            for f in &findings {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {}", icon::WARNING, f.message))
                            .size(11.5)
                            .color(if f.level == trino_scene::ui::Level::Error {
                                c::ERROR
                            } else {
                                c::WARN
                            }),
                    )
                    .wrap(),
                );
            }
            if findings.is_empty() && (keys.key.is_some() || keys.important == Some(true)) {
                ui.label(
                    RichText::new(format!("{} caption checks pass", icon::CHECK))
                        .size(11.5)
                        .color(c::OK),
                );
            }
        },
    );
    if let Some(k) = new_key {
        match app.localization.add_key_everywhere(&k) {
            Ok(()) => {
                app.audio.new_caption_key = None;
                cmds.push(Cmd::SetAudioKey(l.clone(), "caption", Some(k)));
            }
            Err(e) => app.log.toast(crate::log::Level::Error, e),
        }
    }
    if preview && let Some(k) = &keys.key {
        let settings = app.access.preview;
        let place = app.audio.caption_place;
        app.access.caption.show(k, &keys, place, &settings);
        app.cmds.push(Cmd::FocusTab(crate::app::Tab::Game));
    }
    app.cmds.extend(cmds);
}
