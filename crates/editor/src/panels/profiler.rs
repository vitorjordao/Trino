//! The Profiler tab (issue #62): timeline traces from
//! `target/traces/*.json|*.log` (`crate::profiler` holds the state).
//!
//! Top: trace A and B (files and sessions), the frame summary. Then the
//! timeline (a row per track and zone depth, frame marks; the wheel zooms
//! around the cursor, a drag pans, double-click or Fit shows it all), and
//! below it, scrolling: one frame as a flame graph, the per-zone table, and
//! with B loaded the diff (B − A, red when B is slower).

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use trino_core::trace::track;
use trino_trace::Session;

use crate::app::App;
use crate::profiler::{TimeView, fmt_delta_us, fmt_us, visible};
use crate::theme::{self, c, icon};

/// Track label gutter, ruler height, row height, gap between tracks.
const GUTTER: f32 = 46.0;
const RULER: f32 = 14.0;
const ROW: f32 = 16.0;
const TRACK_GAP: f32 = 3.0;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let p = &mut app.profiler;
    p.poll(std::time::Instant::now());
    // Polling needs frames: the file may change while nobody moves.
    ui.ctx().request_repaint_after(crate::profiler::POLL);
    pickers(app, ui);
    let p = &mut app.profiler;
    if let Some(e) = &p.error {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{} {e}", icon::WARNING))
                    .size(11.5)
                    .color(c::ERROR),
            )
            .truncate(),
        );
    }
    if p.session().is_none() {
        theme::empty_state(
            ui,
            icon::PULSE,
            "No trace open",
            "cargo xtask run pc --trace or test n64 write target/traces/",
        );
        return;
    }
    summary(app, ui);
    // The timeline keeps the wheel: it sits outside the scroll area.
    let lanes = app.profiler.lanes();
    let rows: f32 = lanes.iter().map(|(_, n)| *n as f32).sum();
    let need = RULER + rows * ROW + lanes.len() as f32 * TRACK_GAP + 4.0;
    let h = need.min((ui.available_height() * 0.42).max(70.0)).max(40.0);
    timeline(app, ui, &lanes, h);
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("profiler-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            frame_view(app, ui, &lanes);
            ui.add_space(6.0);
            // Comparing is why B is open: the diff comes first, beside the
            // zones when there is room.
            if app.profiler.b.is_some() && ui.available_width() >= 1000.0 {
                ui.columns(2, |cols| {
                    let [left, right] = cols else {
                        return;
                    };
                    diff_table(app, left);
                    zone_table(app, right);
                });
            } else {
                diff_table(app, ui);
                ui.add_space(6.0);
                zone_table(app, ui);
            }
        });
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Trace A / B: file and session combos, refresh and fit.
fn pickers(app: &mut App, ui: &mut egui::Ui) {
    let p = &mut app.profiler;
    let files = p.files.clone();
    let combo_w = ((ui.available_width() - 120.0) / 2.0).clamp(90.0, 240.0);
    let mut open_a = None;
    let mut open_b: Option<Option<std::path::PathBuf>> = None;
    let mut pick_a = None;
    let mut pick_b = None;
    let (mut refresh, mut fit) = (false, false);
    let has_session = p.session().is_some();
    for which in [0, 1] {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if which == 0 { "A" } else { "B" })
                    .family(theme::bold())
                    .color(if which == 0 {
                        c::ACCENT_TEXT
                    } else {
                        c::PREFAB
                    }),
            );
            let loaded = if which == 0 { &p.a } else { &p.b };
            let current = loaded
                .as_ref()
                .map_or_else(|| "none".to_string(), |l| l.file_name());
            theme::fixed_width(ui, combo_w, |ui| {
                egui::ComboBox::from_id_salt(("profiler-file", which))
                    .width(combo_w - 8.0)
                    .truncate()
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if which == 1 && ui.selectable_label(p.b.is_none(), "none").clicked() {
                            open_b = Some(None);
                        }
                        for f in &files {
                            let on = loaded.as_ref().is_some_and(|l| &l.path == f);
                            if ui.selectable_label(on, file_label(f)).clicked() {
                                if which == 0 {
                                    open_a = Some(f.clone());
                                } else {
                                    open_b = Some(Some(f.clone()));
                                }
                            }
                        }
                    })
                    .response
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::ComboBox,
                            true,
                            if which == 0 { "Trace A" } else { "Trace B" },
                        )
                    });
            });
            let sessions: Vec<String> = loaded
                .as_ref()
                .map(|l| l.trace.sessions.iter().map(|s| s.name.clone()).collect())
                .unwrap_or_default();
            let chosen = if which == 0 {
                Some(p.session_a)
            } else {
                p.session_b_index()
            };
            if !sessions.is_empty() {
                let text = chosen
                    .and_then(|i| sessions.get(i))
                    .cloned()
                    .unwrap_or_default();
                theme::fixed_width(ui, combo_w, |ui| {
                    egui::ComboBox::from_id_salt(("profiler-session", which))
                        .width(combo_w - 8.0)
                        .truncate()
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            for (i, s) in sessions.iter().enumerate() {
                                if ui.selectable_label(chosen == Some(i), s).clicked() {
                                    if which == 0 {
                                        pick_a = Some(i);
                                    } else {
                                        pick_b = Some(i);
                                    }
                                }
                            }
                        })
                        .response
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::ComboBox,
                                true,
                                if which == 0 { "Session A" } else { "Session B" },
                            )
                        });
                });
            }
            if which == 0 {
                if theme::named_icon_button(ui, icon::ARROWS_CLOCKWISE, "Refresh trace list")
                    .clicked()
                {
                    refresh = true;
                }
                if has_session
                    && theme::named_icon_button(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, "Fit")
                        .clicked()
                {
                    fit = true;
                }
            }
            if let Some(l) = loaded
                && l.changed
                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3))
            {
                theme::pill(ui, "live", c::BG0, c::PLAY);
            }
        });
    }
    if refresh {
        p.refresh_files();
    }
    if fit {
        p.fit();
    }
    if let Some(f) = open_a {
        p.open_a(&f, None);
    }
    match open_b {
        Some(Some(f)) => p.open_b(&f, None),
        Some(None) => p.close_b(),
        None => {}
    }
    if let Some(i) = pick_a {
        p.set_session_a(i);
    }
    if let Some(i) = pick_b {
        p.session_b = Some(i);
    }
    for l in [&p.a, &p.b].into_iter().flatten() {
        if let Some(e) = &l.error {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("reload failed, showing the last good trace: {e}"))
                        .size(11.0)
                        .color(c::TEXT_FAINT),
                )
                .truncate(),
            );
        }
    }
}

fn summary(app: &mut App, ui: &mut egui::Ui) {
    let p = &mut app.profiler;
    let frames_marked = p.session().map_or(0, |s| s.frames.len());
    let Some(st) = p.stats() else {
        return;
    };
    let mut text = format!("{} ({})", st.session, st.platform);
    if st.frames.count > 0 {
        text += &format!(
            " · {} frames, mean {}, p95 {}, max {}",
            st.frames.count,
            fmt_us(st.frames.mean_us),
            fmt_us(st.frames.p95_us as f64),
            fmt_us(st.frames.max_us as f64)
        );
    } else {
        text += &format!(" · {frames_marked} frame marks");
    }
    let lost = st.dropped;
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(c::TEXT_WEAK)).truncate());
    if lost > 0 {
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "{} {lost} events lost (ring full or busy)",
                    icon::WARNING
                ))
                .size(11.5)
                .color(c::WARN),
            )
            .truncate(),
        );
    }
}

/// A zone name's color: a stable hue per name.
fn zone_color(name: &str) -> Color32 {
    let h = name.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    });
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.45, 0.62, 1.0).into()
}

/// Paint `s`'s zones over `view` into `rect`: a row per (track, depth), the
/// track names in a gutter, a ruler on top (times from `origin`), frame
/// marks when `marks`. Returns the hovered zone's text.
#[allow(clippy::too_many_arguments)]
fn paint_tracks(
    ui: &egui::Ui,
    rect: Rect,
    s: &Session,
    lanes: &[(u8, u8)],
    view: TimeView,
    origin: f64,
    marks: bool,
    hover: Option<Pos2>,
) -> Option<String> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, c::BG1);
    let left = rect.left() + GUTTER;
    let width = (rect.right() - left - 4.0).max(1.0);
    let rows: f32 = lanes.iter().map(|(_, n)| *n as f32).sum::<f32>().max(1.0);
    let row = ((rect.height() - RULER - TRACK_GAP * lanes.len() as f32) / rows).clamp(3.0, ROW);
    let small = egui::FontId::proportional(10.0);
    // Ruler: about one tick per 90 px, on a 1-2-5 step.
    let target = view.width() * 90.0 / width as f64;
    let mag = 10f64.powf(target.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= target)
        .unwrap_or(10.0 * mag);
    let mut t = ((view.t0 - origin) / step).ceil() * step + origin;
    while t <= view.t1 {
        let x = view.x_of(t, left, width);
        painter.line_segment(
            [
                Pos2::new(x, rect.top() + RULER - 4.0),
                Pos2::new(x, rect.bottom()),
            ],
            Stroke::new(1.0, c::BG2),
        );
        painter.text(
            Pos2::new(x + 2.0, rect.top() + 1.0),
            egui::Align2::LEFT_TOP,
            fmt_us(t - origin),
            small.clone(),
            c::TEXT_FAINT,
        );
        t += step;
    }
    if marks {
        for f in &s.frames {
            let t = f.ts as f64;
            if t >= view.t0 && t <= view.t1 {
                let x = view.x_of(t, left, width);
                painter.line_segment(
                    [
                        Pos2::new(x, rect.top() + RULER),
                        Pos2::new(x, rect.bottom()),
                    ],
                    Stroke::new(1.0, c::ACCENT.gamma_multiply(0.7)),
                );
            }
        }
    }
    let mut hovered = None;
    let mut y = rect.top() + RULER;
    let font = egui::FontId::proportional(10.5);
    for &(tr, n) in lanes {
        let track_h = n as f32 * row;
        painter.text(
            Pos2::new(rect.left() + 6.0, y + 1.0),
            egui::Align2::LEFT_TOP,
            track::name(tr),
            small.clone(),
            c::TEXT_WEAK,
        );
        // Zones under a pixel merge with their neighbour on the same row.
        let mut last_x = vec![f32::NEG_INFINITY; n as usize];
        for z in visible(&s.zones, view.t0, view.t1).filter(|z| z.track == tr) {
            let x0 = view.x_of(z.start as f64, left, width).max(left);
            let x1 = view
                .x_of(z.end() as f64, left, width)
                .min(left + width)
                .max(x0 + 1.0);
            let d = (z.depth as usize).min(n as usize - 1);
            if x1 - x0 < 1.5 && x1 <= last_x[d] + 1.0 {
                continue;
            }
            last_x[d] = x1;
            let r = Rect::from_min_max(
                Pos2::new(x0, y + d as f32 * row),
                Pos2::new(x1, y + (d + 1) as f32 * row - 1.0),
            );
            let name = s.name_of(z.name);
            painter.rect_filled(r, 2.0, zone_color(name));
            if r.width() > 28.0 && row >= 11.0 {
                let label = format!("{name} {}", fmt_us(z.dur as f64));
                let galley = painter.layout_no_wrap(label, font.clone(), Color32::BLACK);
                let text = if galley.size().x + 6.0 <= r.width() {
                    galley
                } else {
                    painter.layout_no_wrap(name.to_string(), font.clone(), Color32::BLACK)
                };
                if text.size().x + 6.0 <= r.width() {
                    painter.galley(
                        Pos2::new(r.left() + 3.0, r.center().y - text.size().y / 2.0),
                        text,
                        Color32::BLACK,
                    );
                }
            }
            if let Some(h) = hover
                && r.contains(h)
            {
                hovered = Some(format!(
                    "{name} ({})\n{} · starts at {} · depth {}",
                    track::name(z.track),
                    fmt_us(z.dur as f64),
                    fmt_us(z.start as f64 - origin),
                    z.depth
                ));
            }
        }
        y += track_h + TRACK_GAP;
    }
    hovered
}

fn timeline(app: &mut App, ui: &mut egui::Ui, lanes: &[(u8, u8)], h: f32) {
    let p = &mut app.profiler;
    let (Some(view), Some(bounds)) = (p.current_view(), p.bounds()) else {
        return;
    };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Timeline"));
    let left = rect.left() + GUTTER;
    let width = (rect.right() - left - 4.0).max(1.0);
    let mut v = view;
    if resp.hovered() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let factor = (scroll.y as f64 * 0.004).exp() * zoom as f64;
        if (factor - 1.0).abs() > 1e-6
            && let Some(at) = resp.hover_pos()
        {
            let frac = ((at.x - left) / width) as f64;
            v = v.zoom_at(frac, factor, bounds);
        }
        if scroll.x != 0.0 {
            v = v.pan(-(scroll.x / width) as f64, bounds);
        }
        // The wheel was ours: no scroll area scrolls with it.
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if resp.dragged() {
        v = v.pan(-(resp.drag_delta().x / width) as f64, bounds);
    }
    if resp.double_clicked() {
        p.fit();
    } else if v != view {
        p.set_view(v);
    }
    let Some(s) = p.session() else {
        return;
    };
    let hover = resp.hover_pos();
    if let Some(text) = paint_tracks(ui, rect, s, lanes, v, bounds.t0, true, hover) {
        resp.on_hover_text_at_pointer(text);
    }
}

/// One frame (`frame_range`) as a flame graph.
fn frame_view(app: &mut App, ui: &mut egui::Ui, lanes: &[(u8, u8)]) {
    let p = &mut app.profiler;
    let count = p.session().map_or(0, |s| s.frames.len());
    if count == 0 {
        ui.label(
            RichText::new("No frame marks in this session")
                .size(12.0)
                .color(c::TEXT_FAINT),
        );
        return;
    }
    p.frame = p.frame.min(count - 1);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{}  Frame", icon::FILM_STRIP))
                .family(theme::bold())
                .color(c::TEXT),
        );
        if theme::named_icon_button(ui, icon::CARET_LEFT, "Previous frame").clicked() {
            p.frame = p.frame.saturating_sub(1);
        }
        let w = (ui.available_width() - 150.0).clamp(60.0, 360.0);
        ui.spacing_mut().slider_width = w;
        let r = ui.add(egui::Slider::new(&mut p.frame, 0..=count - 1).show_value(false));
        let _ = r;
        if theme::named_icon_button(ui, icon::CARET_RIGHT, "Next frame").clicked() {
            p.frame = (p.frame + 1).min(count - 1);
        }
        let s = p.session().unwrap();
        let (from, to) = s.frame_range(p.frame).unwrap_or((0, 0));
        let mark = s.frames[p.frame].frame;
        ui.add(
            egui::Label::new(
                RichText::new(format!("#{mark} · {}", fmt_us((to - from) as f64)))
                    .size(12.0)
                    .color(c::TEXT_WEAK),
            )
            .truncate(),
        );
    });
    let s = p.session().unwrap();
    let Some((from, to)) = s.frame_range(p.frame) else {
        return;
    };
    let rows: f32 = lanes.iter().map(|(_, n)| *n as f32).sum();
    let h = (RULER + rows * ROW + lanes.len() as f32 * TRACK_GAP + 4.0).min(220.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Frame flame graph")
    });
    let view = TimeView::fit(from, to);
    if let Some(text) = paint_tracks(
        ui,
        rect,
        s,
        lanes,
        view,
        from as f64,
        false,
        resp.hover_pos(),
    ) {
        resp.on_hover_text_at_pointer(text);
    }
}

/// A row of fixed-width cells: the first left-aligned and truncated, the
/// rest right-aligned.
fn row(ui: &mut egui::Ui, widths: &[f32], cells: &[(String, Color32)], strong: bool) {
    ui.horizontal(|ui| {
        for (i, ((text, color), w)) in cells.iter().zip(widths).enumerate() {
            let mut rt = RichText::new(text).size(12.0).color(*color);
            if strong {
                rt = rt.family(theme::bold());
            }
            let layout = if i == 0 {
                egui::Layout::left_to_right(egui::Align::Center)
            } else {
                egui::Layout::right_to_left(egui::Align::Center)
            };
            ui.allocate_ui_with_layout(Vec2::new(*w, 16.0), layout, |ui| {
                ui.set_min_width(*w);
                ui.set_max_width(*w);
                ui.add(egui::Label::new(rt).truncate());
            });
        }
    });
}

/// Column widths: the first takes what the numbers leave.
fn widths(ui: &egui::Ui, numbers: usize, num_w: f32) -> Vec<f32> {
    let gap = ui.spacing().item_spacing.x;
    let total = ui.available_width() - 4.0;
    let first = (total - numbers as f32 * (num_w + gap) - gap).max(60.0);
    std::iter::once(first)
        .chain(std::iter::repeat_n(num_w, numbers))
        .collect()
}

fn zone_table(app: &mut App, ui: &mut egui::Ui) {
    theme::section_title(ui, icon::LIST_BULLETS, "Zones");
    let wide = ui.available_width() >= 620.0;
    let Some(st) = app.profiler.stats() else {
        return;
    };
    let head: &[&str] = if wide {
        &["zone", "track", "count", "mean", "p95", "max", "per frame"]
    } else {
        &["zone", "mean", "p95", "per frame"]
    };
    let w = widths(ui, head.len() - 1, 66.0);
    row(
        ui,
        &w,
        &head
            .iter()
            .map(|h| (h.to_string(), c::TEXT_FAINT))
            .collect::<Vec<_>>(),
        false,
    );
    for z in &st.zones {
        let n = |v: f64| (fmt_us(v), c::TEXT_WEAK);
        let mut cells = vec![(z.name.clone(), c::TEXT)];
        if wide {
            cells.push((track::name(z.track).to_string(), c::TEXT_FAINT));
            cells.push((z.count.to_string(), c::TEXT_WEAK));
        }
        cells.push(n(z.mean_us));
        cells.push(n(z.p95_us as f64));
        if wide {
            cells.push(n(z.max_us as f64));
        }
        cells.push(n(z.per_frame_us));
        row(ui, &w, &cells, false);
    }
    if !st.counters.is_empty() {
        for cn in &st.counters {
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "counter {}: mean {:.1}, max {} ({} samples)",
                        cn.name, cn.mean, cn.max, cn.samples
                    ))
                    .size(11.5)
                    .color(c::TEXT_FAINT),
                )
                .truncate(),
            );
        }
    }
}

/// The color of a change: red when B is slower, green when faster, faint
/// within 2 %.
fn delta_color(delta: f64, pct: Option<f64>) -> Color32 {
    let small = pct.map_or(delta.abs() < 0.5, |p| p.abs() < 2.0);
    if small {
        c::TEXT_FAINT
    } else if delta > 0.0 {
        c::ERROR
    } else {
        c::OK
    }
}

fn diff_table(app: &mut App, ui: &mut egui::Ui) {
    if app.profiler.b.is_none() {
        return;
    }
    let wide = ui.available_width() >= 620.0;
    let Some(d) = app.profiler.diff() else {
        ui.label(
            RichText::new("Trace B has no session to compare")
                .size(12.0)
                .color(c::TEXT_FAINT),
        );
        return;
    };
    theme::section_title(ui, icon::ARROWS_LEFT_RIGHT, "Diff (B − A)");
    let fd = d.b.frames.mean_us - d.a.frames.mean_us;
    let fpct = (d.a.frames.mean_us > 0.0).then(|| fd / d.a.frames.mean_us * 100.0);
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "{} vs {}: frame mean {} to {} ({}{})",
                d.b.session,
                d.a.session,
                fmt_us(d.a.frames.mean_us),
                fmt_us(d.b.frames.mean_us),
                fmt_delta_us(fd),
                fpct.map_or(String::new(), |p| format!(", {p:+.0}%"))
            ))
            .size(12.0)
            .color(delta_color(fd, fpct)),
        )
        .truncate(),
    );
    let head: &[&str] = if wide {
        &[
            "zone", "track", "A mean", "B mean", "Δ mean", "%", "Δ p95", "Δ/frame",
        ]
    } else {
        &["zone", "Δ mean", "%", "Δ p95"]
    };
    let w = widths(ui, head.len() - 1, 66.0);
    row(
        ui,
        &w,
        &head
            .iter()
            .map(|h| (h.to_string(), c::TEXT_FAINT))
            .collect::<Vec<_>>(),
        false,
    );
    for z in &d.zones {
        let md = z.mean_delta();
        let pct = z.mean_pct();
        let col = delta_color(md, pct);
        let opt = |v: Option<f64>| v.map_or("-".to_string(), fmt_us);
        let mut cells = vec![(z.name.clone(), c::TEXT)];
        if wide {
            cells.push((track::name(z.track).to_string(), c::TEXT_FAINT));
            cells.push((opt(z.a.as_ref().map(|s| s.mean_us)), c::TEXT_WEAK));
            cells.push((opt(z.b.as_ref().map(|s| s.mean_us)), c::TEXT_WEAK));
        }
        cells.push((fmt_delta_us(md), col));
        cells.push((
            if z.a.is_none() {
                "new".to_string()
            } else if z.b.is_none() {
                "gone".to_string()
            } else {
                pct.map_or_else(|| "-".to_string(), |p| format!("{p:+.0}%"))
            },
            col,
        ));
        let p95 = z.p95_delta() as f64;
        cells.push((fmt_delta_us(p95), delta_color(p95, None)));
        if wide {
            let pf = z.per_frame_delta();
            cells.push((fmt_delta_us(pf), delta_color(pf, None)));
        }
        row(ui, &w, &cells, false);
    }
}
