//! Editor log (Console panel) and toasts (transient notifications that slide
//! in at the bottom-right and fade out).

use std::collections::VecDeque;
use std::time::Instant;

use eframe::egui::{self, Align2, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};

use crate::theme::{self, c, icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

impl Level {
    pub fn color(self) -> Color32 {
        match self {
            Level::Info => c::TEXT_WEAK,
            Level::Ok => c::OK,
            Level::Warn => c::WARN,
            Level::Error => c::ERROR,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Level::Info => icon::INFO,
            Level::Ok => icon::CHECK_CIRCLE,
            Level::Warn => icon::WARNING,
            Level::Error => icon::X_CIRCLE,
        }
    }
}

pub struct Entry {
    pub at: Instant,
    pub level: Level,
    pub text: String,
    /// A button beside the entry (issue #68: a damaged baked asset offers
    /// to re-bake it).
    pub action: Option<Action>,
}

/// What an entry's button does; the app runs it ([`Log::take_action`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Bake the assets again (a damaged baked file is rewritten from its
    /// master) and reload them. Names the asset that failed.
    Rebake { logical: String },
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Rebake { .. } => "Re-bake",
        }
    }
}

struct Toast {
    born: Instant,
    level: Level,
    text: String,
}

const TOAST_SECS: f32 = 3.2;
const MAX_ENTRIES: usize = 2000;

pub struct Log {
    pub entries: VecDeque<Entry>,
    toasts: VecDeque<Toast>,
    start: Instant,
    pub filter: [bool; 4],
    pub search: String,
    echo: bool,
    /// An entry's button clicked this frame.
    clicked: Option<Action>,
}

impl Log {
    pub fn new() -> Self {
        Log {
            entries: VecDeque::new(),
            toasts: VecDeque::new(),
            start: Instant::now(),
            filter: [true; 4],
            search: String::new(),
            clicked: None,
            echo: std::env::var("TRINO_EDITOR_ECHO_LOG").is_ok_and(|v| v == "1"),
        }
    }

    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        self.push_with(level, text, None);
    }

    /// An entry with a button ([`Action`]).
    pub fn push_with(&mut self, level: Level, text: impl Into<String>, action: Option<Action>) {
        let text = text.into();
        // TRINO_EDITOR_ECHO_LOG=1 mirrors the console to stderr — lets
        // scripts and AI agents observe a running editor.
        if self.echo {
            eprintln!("[{level:?}] {text}");
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            at: Instant::now(),
            level,
            text,
            action,
        });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Level::Info, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push(Level::Warn, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    /// Log + show a toast.
    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        self.push(level, text.clone());
        if self.toasts.len() >= 4 {
            self.toasts.pop_front();
        }
        self.toasts.push_back(Toast {
            born: Instant::now(),
            level,
            text,
        });
    }

    /// The button an entry's click asked for, once.
    pub fn take_action(&mut self) -> Option<Action> {
        self.clicked.take()
    }

    pub fn last(&self) -> Option<&Entry> {
        self.entries.back()
    }

    pub fn counts(&self) -> (usize, usize) {
        let warns = self
            .entries
            .iter()
            .filter(|e| e.level == Level::Warn)
            .count();
        let errors = self
            .entries
            .iter()
            .filter(|e| e.level == Level::Error)
            .count();
        (warns, errors)
    }

    /// Paint live toasts; returns true while any is animating.
    pub fn show_toasts(&mut self, ctx: &egui::Context) -> bool {
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < TOAST_SECS);
        if self.toasts.is_empty() {
            return false;
        }
        let screen = ctx.content_rect();
        let mut y = screen.max.y - 40.0;
        for (i, t) in self.toasts.iter().enumerate().rev() {
            let age = t.born.elapsed().as_secs_f32();
            // Ease in (slide from the right) and fade out at the end.
            let appear = (age / 0.22).min(1.0);
            let ease = 1.0 - (1.0 - appear).powi(3);
            let fade = ((TOAST_SECS - age) / 0.5).clamp(0.0, 1.0);
            let alpha = ease.min(fade);
            let offset = (1.0 - ease) * 60.0;
            let pos = egui::pos2(screen.max.x - 16.0 + offset, y);
            let area = egui::Area::new(egui::Id::new(("toast", i, t.born)))
                .order(egui::Order::Tooltip)
                .anchor(Align2::RIGHT_BOTTOM, pos - screen.right_bottom())
                .interactable(false)
                .show(ctx, |ui| {
                    ui.set_opacity(alpha);
                    egui::Frame::new()
                        .fill(c::BG2)
                        .stroke(Stroke::new(1.0, t.level.color().gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(14, 10))
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 20,
                            spread: 0,
                            color: Color32::from_black_alpha(110),
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(t.level.glyph())
                                        .size(16.0)
                                        .color(t.level.color()),
                                );
                                ui.label(RichText::new(&t.text).color(c::TEXT));
                            });
                        });
                });
            y -= area.response.rect.height() + 8.0;
        }
        true
    }

    /// The Console panel.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (i, level) in [Level::Info, Level::Ok, Level::Warn, Level::Error]
                .into_iter()
                .enumerate()
            {
                let n = self.entries.iter().filter(|e| e.level == level).count();
                let label = format!("{} {n}", level.glyph());
                if theme::toggle_icon(ui, self.filter[i], &label, &format!("{level:?}")).clicked() {
                    self.filter[i] = !self.filter[i];
                }
            }
            ui.add_space(6.0);
            let w = (ui.available_width() - 70.0).max(80.0);
            ui.allocate_ui(Vec2::new(w, 24.0), |ui| {
                theme::search_box(ui, &mut self.search, "Filter log…");
            });
            if theme::icon_button(ui, icon::BROOM, "Clear console").clicked() {
                self.entries.clear();
            }
            if theme::icon_button(ui, icon::COPY, "Copy all").clicked() {
                let text: Vec<String> = self.entries.iter().map(|e| e.text.clone()).collect();
                ui.ctx().copy_text(text.join("\n"));
            }
        });
        ui.add_space(4.0);
        let needle = self.search.to_lowercase();
        let start = self.start;
        let filter = self.filter;
        let mut clicked = None;
        egui::Frame::new()
            .fill(c::BG0)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for e in self.entries.iter().filter(|e| {
                            let idx = match e.level {
                                Level::Info => 0,
                                Level::Ok => 1,
                                Level::Warn => 2,
                                Level::Error => 3,
                            };
                            filter[idx]
                                && (needle.is_empty() || e.text.to_lowercase().contains(&needle))
                        }) {
                            ui.horizontal_wrapped(|ui| {
                                let secs = e.at.duration_since(start).as_secs_f32();
                                ui.label(
                                    RichText::new(format!("{:>7.2}", secs))
                                        .monospace()
                                        .color(c::TEXT_FAINT),
                                );
                                ui.label(RichText::new(e.level.glyph()).color(e.level.color()));
                                let color = match e.level {
                                    Level::Info => c::TEXT,
                                    other => other.color(),
                                };
                                ui.label(RichText::new(&e.text).monospace().color(color));
                                if let Some(a) = &e.action
                                    && ui
                                        .add(egui::Button::new(
                                            RichText::new(format!(
                                                "{} {}",
                                                icon::ARROWS_CLOCKWISE,
                                                a.label()
                                            ))
                                            .color(c::ACCENT),
                                        ))
                                        .clicked()
                                {
                                    clicked = Some(a.clone());
                                }
                            });
                        }
                    });
            });
        if clicked.is_some() {
            self.clicked = clicked;
        }
    }
}
