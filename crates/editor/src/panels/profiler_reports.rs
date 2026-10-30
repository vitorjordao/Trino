//! The Profiler's report views (issue #79): **Reports** — the editor's own
//! PC frame live (the Game view), then a console perf report from
//! `target/perf/*.json` (what every `cargo xtask test n64` writes: stages,
//! T&L work, backend counters, zones, the RSP fit per overlay, the CPU
//! samples), optionally against a second one; **A/B** — a `cargo xtask perf
//! ab` report: per scene and stage, each arm's mean and its spread over the
//! code-layout paddings, one dot per padding. `cargo xtask perf report`
//! prints the same files.

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};
use trino_trace::perf::{AbReport, PerfReport, STAGES, WORK};

use crate::app::App;
use crate::profiler::{LiveFrame, fmt_delta_us, fmt_us};
use crate::theme::{self, c, icon};

fn label(ui: &mut egui::Ui, text: impl Into<String>, color: Color32, size: f32) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(size).color(color)).truncate());
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The live PC row: the Game view's last frames.
fn live(app: &mut App, ui: &mut egui::Ui) {
    let s = &app.game_view.stats;
    let f = LiveFrame {
        ms: s.ms,
        stats: s.frame,
        times: s.times,
    };
    let live = &mut app.profiler.live;
    if live.last() != Some(&f) {
        live.push(f);
    }
    theme::section_title(ui, icon::GAUGE, "PC frame (Game view, live)");
    let Some(last) = live.last().copied() else {
        label(ui, "The Game view hasn't drawn yet.", c::TEXT_FAINT, 11.5);
        return;
    };
    let (mean, max) = live.ms();
    label(
        ui,
        format!(
            "{:.2} ms (mean {:.2}, max {:.2} over {} frames) · {} draws, {} tris, {} on the GPU T&L",
            last.ms,
            mean,
            max,
            live.frames.len(),
            last.stats.models,
            last.stats.tris,
            last.stats.tnl.pc_gpu_draws
        ),
        c::TEXT_WEAK,
        12.0,
    );
    let t = last.times;
    label(
        ui,
        format!(
            "CPU: T&L {}, batches {}, submit {} · {} draw calls, {} pipeline switches, {} KB uploaded",
            fmt_us(t.tnl_us as f64),
            fmt_us(t.batch_us as f64),
            fmt_us(t.submit_us as f64),
            t.draw_calls,
            t.pipeline_switches,
            t.upload_bytes / 1024
        ),
        c::TEXT_FAINT,
        11.5,
    );
    // The frame times as a strip of bars.
    let w = ui.available_width().min(480.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, c::BG2);
    let top = live.frames.iter().map(|f| f.ms).fold(16.7f32, f32::max);
    let n = live.frames.len().max(1) as f32;
    let bw = rect.width() / crate::profiler::LIVE_FRAMES as f32;
    for (i, f) in live.frames.iter().enumerate() {
        let h = rect.height() * (f.ms / top).min(1.0);
        let x = rect.left() + (crate::profiler::LIVE_FRAMES as f32 - n + i as f32) * bw;
        let r = egui::Rect::from_min_max(
            egui::pos2(x, rect.bottom() - h),
            egui::pos2(x + bw.max(1.0) - 0.5, rect.bottom()),
        );
        painter.rect_filled(r, 0.0, if f.ms > 16.7 { c::WARN } else { c::PLAY });
    }
    let y60 = rect.bottom() - rect.height() * (16.7 / top);
    painter.hline(rect.x_range(), y60, Stroke::new(1.0, c::TEXT_FAINT));
}

fn report_picker(app: &mut App, ui: &mut egui::Ui) {
    let r = &mut app.profiler.reports;
    let files = r.files.clone();
    let combo_w = ((ui.available_width() - 60.0) / 2.0).clamp(90.0, 260.0);
    let mut open: Option<(std::path::PathBuf, bool)> = None;
    let mut close_b = false;
    let mut refresh = false;
    ui.horizontal(|ui| {
        for b in [false, true] {
            let current = if b { &r.b } else { &r.a }
                .as_ref()
                .map_or_else(|| "none".to_string(), |(p, _)| file_name(p));
            ui.label(
                RichText::new(if b { "B" } else { "A" })
                    .family(theme::bold())
                    .color(if b { c::PREFAB } else { c::ACCENT_TEXT }),
            );
            theme::fixed_width(ui, combo_w, |ui| {
                egui::ComboBox::from_id_salt(("perf-report", b))
                    .width(combo_w - 8.0)
                    .truncate()
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if b && ui.selectable_label(r.b.is_none(), "none").clicked() {
                            close_b = true;
                        }
                        for f in &files {
                            if ui.selectable_label(false, file_name(f)).clicked() {
                                open = Some((f.clone(), b));
                            }
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::ComboBox,
                            true,
                            if b { "Report B" } else { "Report A" },
                        )
                    });
            });
        }
        if theme::named_icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh report list").clicked() {
            refresh = true;
        }
    });
    if refresh {
        r.refresh();
    }
    if close_b {
        r.close_b();
    }
    if let Some((f, b)) = open {
        r.open(&f, b);
    }
}

/// Fixed-width cells, the first left-aligned (the Profiler's table rows).
fn row(ui: &mut egui::Ui, first: f32, num: f32, cells: &[(String, Color32)], strong: bool) {
    ui.horizontal(|ui| {
        for (i, (text, color)) in cells.iter().enumerate() {
            let w = if i == 0 { first } else { num };
            let mut rt = RichText::new(text).size(12.0).color(*color);
            if strong {
                rt = rt.family(theme::bold());
            }
            let layout = if i == 0 {
                egui::Layout::left_to_right(egui::Align::Center)
            } else {
                egui::Layout::right_to_left(egui::Align::Center)
            };
            ui.allocate_ui_with_layout(Vec2::new(w, 16.0), layout, |ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                ui.add(egui::Label::new(rt).truncate());
            });
        }
    });
}

fn first_width(ui: &egui::Ui, numbers: usize, num: f32) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    (ui.available_width() - 4.0 - numbers as f32 * (num + gap) - gap).max(70.0)
}

fn delta_color(d: f64, base: f64) -> Color32 {
    let pct = if base > 0.0 { 100.0 * d / base } else { 0.0 };
    if pct.abs() < 2.0 {
        c::TEXT_FAINT
    } else if d > 0.0 {
        c::ERROR
    } else {
        c::OK
    }
}

/// Name, A, and with B: B and B − A.
fn compare_rows(ui: &mut egui::Ui, rows: &[(String, u64, Option<u64>)], us: bool) {
    let b = rows.iter().any(|r| r.2.is_some());
    let n = if b { 3 } else { 1 };
    let num = 74.0;
    let first = first_width(ui, n, num);
    let fmt = |v: u64| if us { fmt_us(v as f64) } else { v.to_string() };
    let mut head = vec![(String::new(), c::TEXT_FAINT), ("A".into(), c::ACCENT_TEXT)];
    if b {
        head.push(("B".into(), c::PREFAB));
        head.push(("B − A".into(), c::TEXT_FAINT));
    }
    row(ui, first, num, &head, false);
    for (name, a, bv) in rows {
        let mut cells = vec![(name.clone(), c::TEXT), (fmt(*a), c::TEXT_WEAK)];
        if b {
            match bv {
                Some(v) => {
                    let d = *v as f64 - *a as f64;
                    cells.push((fmt(*v), c::TEXT_WEAK));
                    cells.push((
                        if us {
                            fmt_delta_us(d)
                        } else if d > 0.0 {
                            format!("+{d}")
                        } else if d < 0.0 {
                            format!("−{}", -d)
                        } else {
                            "±0".into()
                        },
                        delta_color(d, *a as f64),
                    ));
                }
                None => {
                    cells.push(("–".into(), c::TEXT_FAINT));
                    cells.push((String::new(), c::TEXT_FAINT));
                }
            }
        }
        row(ui, first, num, &cells, false);
    }
}

fn report_view(ui: &mut egui::Ui, a: &PerfReport, b: Option<&PerfReport>) {
    label(
        ui,
        format!(
            "{} {} · {} build{} · frame {}{}",
            a.target,
            a.scene,
            a.build,
            if a.pad > 0 {
                format!(", pad {}", a.pad)
            } else {
                String::new()
            },
            fmt_us(a.frame_us() as f64),
            b.map_or(String::new(), |b| format!(
                " · B: {} {} ({}) {}",
                b.target,
                b.scene,
                b.build,
                fmt_us(b.frame_us() as f64)
            ))
        ),
        c::TEXT_WEAK,
        12.0,
    );
    // The stages as one bar: where the frame goes.
    let w = ui.available_width().min(520.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 14.0), Sense::hover());
    let total = a.frame_us().max(1) as f32;
    let mut x = rect.left();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, c::BG2);
    for st in ["tnl", "sort", "submit", "sprites", "end"] {
        let v = a.stage(st) as f32;
        let ww = rect.width() * (v / total).min(1.0);
        let col = match st {
            "tnl" => c::ACCENT,
            "sort" => c::MODEL,
            "submit" => c::PREFAB,
            "sprites" => c::SPRITE,
            _ => c::TEXT_FAINT,
        };
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(ww, rect.height())),
            0.0,
            col,
        );
        x += ww;
    }
    ui.add_space(2.0);
    theme::section_title(ui, icon::TIMER, "Stages (µs per frame)");
    let stage_rows: Vec<_> = STAGES
        .iter()
        .map(|st| (st.to_string(), a.stage(st), b.map(|b| b.stage(st))))
        .collect();
    compare_rows(ui, &stage_rows, true);
    theme::section_title(ui, icon::CUBE, "T&L work (per frame)");
    let get = |r: &PerfReport, k: &str| r.fields.get(k).copied().unwrap_or(0);
    let work_rows: Vec<_> = WORK
        .iter()
        .map(|w| (w.to_string(), get(a, w), b.map(|b| get(b, w))))
        .collect();
    compare_rows(ui, &work_rows, false);
    if !a.counters.is_empty() {
        theme::section_title(ui, icon::LIST_NUMBERS, "Backend counters (per frame)");
        let rows: Vec<_> = a
            .counters
            .iter()
            .map(|(k, v)| (k.clone(), *v, b.and_then(|b| b.counters.get(k).copied())))
            .collect();
        compare_rows(ui, &rows, false);
    } else {
        label(
            ui,
            "No backend counters: a plain build (`cargo xtask test n64 --trace` counts them).",
            c::TEXT_FAINT,
            11.5,
        );
    }
    if let Some(z) = a.zones.as_ref().and_then(|z| z["zones"].as_array()) {
        theme::section_title(ui, icon::LIST_BULLETS, "Zones (µs per frame)");
        let zb = b
            .and_then(|b| b.zones.as_ref())
            .and_then(|z| z["zones"].as_array());
        let per = |zs: &[trino_trace::perf::Value], name: &str| {
            zs.iter()
                .find(|z| z["name"] == name)
                .and_then(|z| z["per_frame_us"].as_f64())
                .map(|v| v.round() as u64)
        };
        let rows: Vec<_> = z
            .iter()
            .filter_map(|zone| {
                let name = zone["name"].as_str()?;
                Some((
                    name.to_string(),
                    zone["per_frame_us"].as_f64()?.round() as u64,
                    zb.and_then(|zb| per(zb, name)),
                ))
            })
            .collect();
        compare_rows(ui, &rows, true);
    }
    if let Some(r) = &a.rsp {
        theme::section_title(ui, icon::CPU, "RSP per command (--prof rsp)");
        label(
            ui,
            format!(
                "µs = per command + per vertex · v + per triangle · t (empty drain {} µs subtracted)",
                r.empty_drain_us
            ),
            c::TEXT_FAINT,
            11.5,
        );
        let num = 70.0;
        let first = first_width(ui, 5, num);
        row(
            ui,
            first,
            num,
            &[
                "overlay", "commands", "mean", "per cmd", "per vert", "per tri",
            ]
            .map(|h| (h.to_string(), c::TEXT_FAINT)),
            false,
        );
        for f in &r.fits {
            row(
                ui,
                first,
                num,
                &[
                    (f.ovl.clone(), c::TEXT),
                    (f.commands.to_string(), c::TEXT_WEAK),
                    (format!("{:.1}", f.mean_us), c::TEXT_WEAK),
                    (format!("{:.1}", f.per_cmd), c::TEXT_WEAK),
                    (format!("{:.3}", f.per_vert), c::TEXT_WEAK),
                    (format!("{:.3}", f.per_tri), c::TEXT_WEAK),
                ],
                false,
            );
        }
    }
    if let Some(p) = &a.samples {
        theme::section_title(
            ui,
            icon::CHART_BAR_HORIZONTAL,
            "CPU samples (--prof sample)",
        );
        label(
            ui,
            format!(
                "{} samples over {} frames, every {} µs; the sampler cost {:.2} µs each ({:.1}% of the time)",
                p.samples, p.frames, p.period_us, p.handler_us, p.overhead_pct
            ),
            c::TEXT_FAINT,
            11.5,
        );
        let num = 60.0;
        let first = first_width(ui, 2, num);
        for f in p.functions.iter().take(20) {
            row(
                ui,
                first,
                num,
                &[
                    (f.name.clone(), c::TEXT),
                    (f.samples.to_string(), c::TEXT_WEAK),
                    (format!("{:.1}%", f.pct), c::TEXT_WEAK),
                ],
                false,
            );
        }
    }
}

pub fn reports(app: &mut App, ui: &mut egui::Ui) {
    live(app, ui);
    ui.add_space(6.0);
    theme::section_title(ui, icon::FILE_TEXT, "Console report");
    report_picker(app, ui);
    let r = &app.profiler.reports;
    if let Some(e) = &r.error {
        label(ui, format!("{} {e}", icon::WARNING), c::ERROR, 11.5);
    }
    let Some((_, a)) = &r.a else {
        theme::empty_state(
            ui,
            icon::FILE_TEXT,
            "No perf report",
            "cargo xtask test n64 writes target/perf/n64-<scene>.json",
        );
        return;
    };
    let (a, b) = (a.clone(), r.b.as_ref().map(|(_, b)| b.clone()));
    report_view(ui, &a, b.as_ref());
}

/// One stage of one scene: each arm's spread on a shared scale, a dot per
/// padding, the mean as a tick.
fn spread_strip(ui: &mut egui::Ui, ab: &AbReport, scene: usize, stage: &str) {
    let sc = &ab.scenes[scene];
    let (lo, hi) = sc
        .arms
        .iter()
        .filter_map(|a| a.stages.get(stage))
        .fold((u64::MAX, 0u64), |(lo, hi), s| {
            (lo.min(s.min), hi.max(s.max))
        });
    if lo > hi {
        return;
    }
    let pad = ((hi - lo) as f32 * 0.08).max(1.0);
    let (lo, hi) = (lo as f32 - pad, hi as f32 + pad);
    let w = ui.available_width().min(560.0);
    let lane = 13.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(w, lane * sc.arms.len() as f32 + 4.0),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, c::BG2);
    let x_of = |v: f32| rect.left() + 64.0 + (rect.width() - 72.0) * (v - lo) / (hi - lo);
    for (i, arm) in sc.arms.iter().enumerate() {
        let Some(s) = arm.stages.get(stage) else {
            continue;
        };
        let y = rect.top() + 2.0 + lane * (i as f32 + 0.5);
        let col = match arm.arm.as_str() {
            "base" => c::TEXT_WEAK,
            "head" => c::ACCENT_TEXT,
            _ => c::WARN,
        };
        painter.text(
            egui::pos2(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            &arm.arm,
            egui::FontId::proportional(10.5),
            col,
        );
        painter.line_segment(
            [
                egui::pos2(x_of(s.min as f32), y),
                egui::pos2(x_of(s.max as f32), y),
            ],
            Stroke::new(1.5, col.gamma_multiply(0.6)),
        );
        for v in &s.per_pad {
            painter.circle_filled(egui::pos2(x_of(*v as f32), y), 2.4, col);
        }
        let m = x_of(s.mean as f32);
        painter.line_segment(
            [egui::pos2(m, y - 5.0), egui::pos2(m, y + 5.0)],
            Stroke::new(2.0, col),
        );
    }
}

pub fn ab(app: &mut App, ui: &mut egui::Ui) {
    let r = &mut app.profiler.reports;
    let files = r.ab_files.clone();
    let mut open = None;
    let mut refresh = false;
    ui.horizontal(|ui| {
        let current =
            r.ab.as_ref()
                .map_or_else(|| "none".to_string(), |(p, _)| file_name(p));
        let w = (ui.available_width() - 40.0).clamp(90.0, 320.0);
        theme::fixed_width(ui, w, |ui| {
            egui::ComboBox::from_id_salt("perf-ab-file")
                .width(w - 8.0)
                .truncate()
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for f in &files {
                        if ui.selectable_label(false, file_name(f)).clicked() {
                            open = Some(f.clone());
                        }
                    }
                })
                .response
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "A/B report")
                });
        });
        if theme::named_icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh A/B list").clicked() {
            refresh = true;
        }
    });
    if refresh {
        r.refresh();
    }
    if let Some(f) = open {
        r.open_ab(&f);
    }
    if let Some(e) = &r.error {
        label(ui, format!("{} {e}", icon::WARNING), c::ERROR, 11.5);
    }
    let Some((_, abr)) = &r.ab else {
        theme::empty_state(
            ui,
            icon::COLUMNS,
            "No A/B report",
            "cargo xtask perf ab --base <rev> --control writes target/perf-ab/ab.json",
        );
        return;
    };
    let abr = abr.clone();
    label(
        ui,
        format!(
            "base {} vs head {} over {} paddings{}",
            abr.base,
            abr.head,
            abr.paddings.len(),
            if abr.control {
                " · control: base + a no-op patch"
            } else {
                ""
            }
        ),
        c::TEXT_WEAK,
        12.0,
    );
    // The scenes as buttons (a wrap of frameless buttons: kittest clicks
    // them).
    let mut pick = None;
    ui.horizontal_wrapped(|ui| {
        for (i, sc) in abr.scenes.iter().enumerate() {
            let on = i == r.ab_scene;
            let text = RichText::new(&sc.scene).size(11.5).color(if on {
                c::ACCENT_TEXT
            } else {
                c::TEXT_WEAK
            });
            if ui
                .add(egui::Button::new(text).frame(on).fill(c::ACCENT_SOFT))
                .clicked()
            {
                pick = Some(i);
            }
        }
    });
    if let Some(i) = pick {
        r.ab_scene = i;
    }
    let i = r.ab_scene.min(abr.scenes.len().saturating_sub(1));
    let Some(sc) = abr.scenes.get(i) else {
        return;
    };
    label(
        ui,
        if sc.same_work {
            format!(
                "{} the T&L work is identical in every arm: a time difference is code speed or layout",
                icon::CHECK
            )
        } else {
            format!(
                "{} the T&L work differs between arms: not a layout question",
                icon::WARNING
            )
        },
        if sc.same_work { c::OK } else { c::WARN },
        11.5,
    );
    let base = sc.arms.iter().find(|a| a.arm == "base");
    for stage in ["frame", "tnl", "sort", "submit", "end"] {
        let mut text = format!("{stage}:");
        for a in &sc.arms {
            if let Some(s) = a.stages.get(stage) {
                text += &format!("  {} {} ({}–{})", a.arm, fmt_us(s.mean), s.min, s.max);
            }
        }
        if let Some(b) = base.and_then(|b| b.stages.get(stage)) {
            for a in sc.arms.iter().filter(|a| a.arm != "base") {
                if let Some(s) = a.stages.get(stage) {
                    text += &format!("  {}−base {}", a.arm, fmt_delta_us(s.mean - b.mean));
                }
            }
        }
        label(ui, text, c::TEXT, 12.0);
        spread_strip(ui, &abr, i, stage);
        ui.add_space(3.0);
    }
}
