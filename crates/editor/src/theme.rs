//! Look & feel: palette, fonts (Inter + JetBrains Mono + Phosphor icons),
//! egui style, dock style, and the small reusable widgets every panel uses
//! (section headers, icon buttons, XYZ rows, pills, empty states).
//!
//! One place to tune the editor's visual identity; panels never hardcode
//! colors.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Shadow, Stroke, TextStyle, Vec2,
};
pub use egui_phosphor::regular as icon;

/// Palette (dark, slate + violet accent).
pub mod c {
    use eframe::egui::Color32;

    pub const BG0: Color32 = Color32::from_rgb(12, 14, 19); // window / deepest
    pub const BG1: Color32 = Color32::from_rgb(19, 22, 30); // panels
    pub const BG2: Color32 = Color32::from_rgb(27, 31, 42); // widgets
    pub const BG3: Color32 = Color32::from_rgb(37, 42, 56); // hovered widgets
    pub const BG4: Color32 = Color32::from_rgb(48, 54, 71); // pressed
    pub const BORDER: Color32 = Color32::from_rgb(40, 45, 60);
    pub const TEXT: Color32 = Color32::from_rgb(222, 225, 232);
    pub const TEXT_WEAK: Color32 = Color32::from_rgb(140, 147, 165);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(92, 99, 118);
    pub const ACCENT: Color32 = Color32::from_rgb(132, 104, 255);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(64, 52, 125);
    pub const ACCENT_TEXT: Color32 = Color32::from_rgb(196, 182, 255);
    pub const PLAY: Color32 = Color32::from_rgb(45, 212, 160);
    pub const WARN: Color32 = Color32::from_rgb(250, 190, 70);
    pub const ERROR: Color32 = Color32::from_rgb(244, 92, 110);
    pub const OK: Color32 = Color32::from_rgb(90, 214, 130);
    pub const AXIS_X: Color32 = Color32::from_rgb(240, 86, 107);
    pub const AXIS_Y: Color32 = Color32::from_rgb(120, 214, 96);
    pub const AXIS_Z: Color32 = Color32::from_rgb(80, 142, 255);
    pub const PREFAB: Color32 = Color32::from_rgb(86, 196, 255);
    pub const MODEL: Color32 = Color32::from_rgb(255, 172, 92);
    pub const SPRITE: Color32 = Color32::from_rgb(240, 120, 200);
    pub const MATERIAL: Color32 = Color32::from_rgb(120, 220, 200);
    /// Solid colliders (wireframes + inspector); triggers use [`TRIGGER`].
    pub const COLLIDER: Color32 = Color32::from_rgb(110, 235, 120);
    pub const TRIGGER: Color32 = Color32::from_rgb(255, 210, 80);
    /// One-way platforms, ramps and mesh colliders (issue #66).
    pub const ONE_WAY: Color32 = Color32::from_rgb(90, 220, 235);
    pub const RAMP: Color32 = Color32::from_rgb(175, 235, 95);
    pub const MESH_COLLIDER: Color32 = Color32::from_rgb(70, 200, 150);
    /// Character bodies and the controller's debug overlay.
    pub const CHARACTER: Color32 = Color32::from_rgb(255, 120, 220);
    /// Interior cells (rooms) and portals (doorways) — see `trino_core::vis`.
    pub const CELL: Color32 = Color32::from_rgb(120, 170, 255);
    pub const PORTAL: Color32 = Color32::from_rgb(255, 140, 90);
}

pub const RADIUS: u8 = 6;

/// Install fonts and style on the context. Call once at startup.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")).into(),
    );
    fonts.font_data.insert(
        "jetbrains".into(),
        FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "phosphor".into(),
        egui_phosphor::Variant::Regular.font_data().into(),
    );
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family = |first: &str| {
        let mut v = vec![first.to_string(), "phosphor".to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family("inter"));
    fonts
        .families
        .insert(FontFamily::Name("bold".into()), family("inter-semibold"));
    fonts
        .families
        .insert(FontFamily::Monospace, family("jetbrains"));
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Heading, FontId::new(16.0, bold())),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(8.0, 6.0);
        s.button_padding = Vec2::new(9.0, 4.0);
        s.interact_size = Vec2::new(36.0, 24.0);
        s.indent = 16.0;
        s.window_margin = Margin::same(12);
        s.menu_margin = Margin::same(6);
        s.combo_width = 140.0;
        s.slider_width = 120.0;
        s.icon_width = 15.0;
        s.icon_spacing = 6.0;
        s.tooltip_width = 320.0;

        style.animation_time = 0.14;
        style.interaction.tooltip_delay = 0.35;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = c::BG1;
        v.window_fill = c::BG1;
        v.faint_bg_color = Color32::from_rgb(23, 26, 35);
        v.extreme_bg_color = c::BG0;
        v.code_bg_color = c::BG0;
        v.text_edit_bg_color = Some(c::BG0);
        v.window_stroke = Stroke::new(1.0, c::BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.hyperlink_color = c::ACCENT_TEXT;
        v.warn_fg_color = c::WARN;
        v.error_fg_color = c::ERROR;
        v.selection.bg_fill = c::ACCENT_SOFT;
        v.selection.stroke = Stroke::new(1.0, c::ACCENT_TEXT);
        v.striped = false;
        v.indent_has_left_vline = true;
        v.collapsing_header_frame = false;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;

        let r = CornerRadius::same(RADIUS);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c::BG1;
        w.noninteractive.weak_bg_fill = c::BG1;
        w.noninteractive.bg_stroke = Stroke::new(1.0, c::BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, c::TEXT_WEAK);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = c::BG2;
        w.inactive.weak_bg_fill = c::BG2;
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(46, 52, 68));
        w.inactive.fg_stroke = Stroke::new(1.0, c::TEXT);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = c::BG3;
        w.hovered.weak_bg_fill = c::BG3;
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 76, 98));
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = c::BG4;
        w.active.weak_bg_fill = c::BG4;
        w.active.bg_stroke = Stroke::new(1.0, c::ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = c::BG3;
        w.open.weak_bg_fill = c::BG3;
        w.open.bg_stroke = Stroke::new(1.0, c::BORDER);
        w.open.fg_stroke = Stroke::new(1.0, c::TEXT);
        w.open.corner_radius = r;
    });
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Dock look: flat tab bar, accent underline on the active tab.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut d = egui_dock::Style::from_egui(style);
    d.main_surface_border_stroke = Stroke::NONE;
    d.dock_area_padding = Some(Margin::same(4));
    d.separator.width = 3.0;
    d.separator.extra = 60.0;
    d.separator.color_idle = c::BG0;
    d.separator.color_hovered = c::ACCENT_SOFT;
    d.separator.color_dragged = c::ACCENT;
    d.tab_bar.bg_fill = c::BG0;
    d.tab_bar.height = 30.0;
    d.tab_bar.hline_color = c::BG0;
    d.tab_bar.corner_radius = CornerRadius {
        nw: 8,
        ne: 8,
        sw: 0,
        se: 0,
    };
    d.tab.spacing = 2.0;
    d.tab.hline_below_active_tab_name = true;
    d.tab.minimum_width = Some(84.0);
    let round_top = CornerRadius {
        nw: 7,
        ne: 7,
        sw: 0,
        se: 0,
    };
    for (t, fill, text) in [
        (&mut d.tab.active, c::BG1, c::TEXT),
        (&mut d.tab.focused, c::BG1, Color32::WHITE),
        (&mut d.tab.active_with_kb_focus, c::BG1, c::TEXT),
        (&mut d.tab.focused_with_kb_focus, c::BG1, Color32::WHITE),
        (&mut d.tab.inactive, c::BG0, c::TEXT_FAINT),
        (&mut d.tab.inactive_with_kb_focus, c::BG0, c::TEXT_FAINT),
        (&mut d.tab.hovered, c::BG2, c::TEXT_WEAK),
    ] {
        t.bg_fill = fill;
        t.text_color = text;
        t.outline_color = Color32::TRANSPARENT;
        t.corner_radius = round_top;
    }
    d.tab.tab_body.inner_margin = Margin::same(8);
    d.tab.tab_body.stroke = Stroke::NONE;
    d.tab.tab_body.corner_radius = CornerRadius {
        nw: 0,
        ne: 8,
        sw: 8,
        se: 8,
    };
    d.overlay.selection_color = c::ACCENT.gamma_multiply(0.35);
    d
}

// ---------------------------------------------------------------- widgets --

/// Small caps-ish section title with an icon.
pub fn section_title(ui: &mut egui::Ui, glyph: &str, text: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(format!("{glyph}  {}", text.to_uppercase()))
            .size(11.0)
            .color(c::TEXT_FAINT)
            .family(bold()),
    );
}

/// A card-like collapsible component section (inspector).
pub fn component<R>(
    ui: &mut egui::Ui,
    id: &str,
    glyph: &str,
    tint: Color32,
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    component_open(ui, id, glyph, tint, title, true, add)
}

/// [`component`] that starts collapsed unless `open`.
pub fn component_open<R>(
    ui: &mut egui::Ui,
    id: &str,
    glyph: &str,
    tint: Color32,
    title: &str,
    open: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let mut out = None;
    egui::Frame::new()
        .fill(c::BG2.gamma_multiply(0.55))
        .stroke(Stroke::new(1.0, c::BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 2.0);
            let header = egui::CollapsingHeader::new(
                RichText::new(format!("{glyph}  {title}"))
                    .color(tint)
                    .family(bold()),
            )
            .id_salt(id)
            .default_open(open);
            header.show(ui, |ui| {
                ui.add_space(4.0);
                out = Some(add(ui));
            });
        });
    ui.add_space(6.0);
    out
}

/// [`icon_button`] whose accessible name is `name` (the tooltip too):
/// screen readers and the UI tests find it by that, not by the glyph.
pub fn named_icon_button(ui: &mut egui::Ui, glyph: &str, name: &str) -> egui::Response {
    let r = icon_button(ui, glyph, name);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    r
}

/// Frameless icon button with hover background and tooltip.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(glyph).size(15.0))
            .frame_when_inactive(false)
            .min_size(Vec2::new(26.0, 24.0)),
    )
    .on_hover_text(tooltip)
}

/// Toggle-able icon button (filled accent when on).
pub fn toggle_icon(ui: &mut egui::Ui, on: bool, glyph: &str, tooltip: &str) -> egui::Response {
    let text =
        RichText::new(glyph)
            .size(15.0)
            .color(if on { Color32::WHITE } else { c::TEXT_WEAK });
    let mut b = egui::Button::new(text).min_size(Vec2::new(28.0, 24.0));
    b = if on {
        b.fill(c::ACCENT_SOFT).stroke(Stroke::new(1.0, c::ACCENT))
    } else {
        b.frame_when_inactive(false)
    };
    ui.add(b).on_hover_text(tooltip)
}

/// Accent-filled primary button.
pub fn primary_button(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text.into())
                .color(Color32::WHITE)
                .family(bold()),
        )
        .fill(c::ACCENT)
        .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// Rounded status pill.
pub fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(11.5).color(fg)))
        .response
}

/// One labeled XYZ row of drag values with colored axis tags.
/// Returns true when any component changed.
pub fn vec3_row(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut [f32; 3],
    speed: f64,
    suffix: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.add_sized(
            [label_width(ui), 22.0],
            egui::Label::new(RichText::new(label).color(c::TEXT_WEAK)).truncate(),
        );
        // label + 3 × (tag + field), with 3 px gaps between all 7 items.
        let w = ((ui.available_width() - 3.0 * 12.0 - 6.0 * 3.0 - 2.0) / 3.0)
            .floor()
            .max(26.0);
        // A field grows past `w` to fit its text: in a narrow panel show
        // fewer decimals and pad less (-10.5 must fit in ~45 px).
        let decimals = if w < 52.0 {
            1
        } else if w < 64.0 {
            2
        } else {
            3
        };
        if w < 64.0 {
            ui.spacing_mut().button_padding.x = 2.0;
        }
        for (i, (axis, col)) in [("X", c::AXIS_X), ("Y", c::AXIS_Y), ("Z", c::AXIS_Z)]
            .into_iter()
            .enumerate()
        {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, 22.0), egui::Sense::hover());
            ui.painter().rect_filled(
                rect.shrink2(Vec2::new(0.0, 4.0)),
                CornerRadius::same(3),
                col.gamma_multiply(0.9),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                axis,
                FontId::new(9.5, bold()),
                c::BG0,
            );
            changed |= ui
                .add_sized(
                    [w, 22.0],
                    egui::DragValue::new(&mut v[i])
                        .speed(speed)
                        .suffix(suffix)
                        .max_decimals(decimals),
                )
                .changed();
        }
    });
    changed
}

/// Lay `add` out in a box exactly `width` wide (in a row). A truncating
/// `ComboBox` sizes its text to the space *available*, not to its
/// `.width()`, so a combo followed by a button needs this box to stop at
/// its width.
pub fn fixed_width<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = width.max(1.0);
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_max_width(width);
            add(ui)
        },
    )
    .inner
}

/// Width of a property row's label: 64 px, less in a narrow panel.
pub fn label_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() * 0.3).clamp(40.0, 64.0).floor()
}

/// Labeled property row: label + widget. The row never grows past the
/// panel: long texts truncate (with "…") and sliders shrink to fit
/// (`layout_audit` checks it at the editor's minimum window size).
pub fn prop<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_sized(
            [label_width(ui), 22.0],
            egui::Label::new(RichText::new(label).color(c::TEXT_WEAK)).truncate(),
        );
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        // A slider is its track plus a value box (~60 px with a suffix).
        ui.spacing_mut().slider_width = (ui.available_width() - 72.0).clamp(30.0, 120.0);
        add(ui)
    })
    .inner
}

/// Centered hint for empty panels.
pub fn empty_state(ui: &mut egui::Ui, glyph: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).max(12.0));
        ui.label(RichText::new(glyph).size(34.0).color(c::TEXT_FAINT));
        ui.add_space(4.0);
        ui.label(RichText::new(title).color(c::TEXT_WEAK).family(bold()));
        ui.label(RichText::new(hint).size(12.0).color(c::TEXT_FAINT));
    });
}

/// Search box with a magnifier icon and a hint.
pub fn search_box(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    egui::Frame::new()
        .fill(c::BG0)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(8, 2))
        .stroke(Stroke::new(1.0, c::BORDER))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(c::TEXT_FAINT));
                let r = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                );
                if !text.is_empty() && icon_button(ui, icon::X, "Clear").clicked() {
                    text.clear();
                }
                r
            })
            .inner
        })
        .inner
}

/// sRGB color button for `[u8; 4]` (with alpha).
pub fn color_rgba(ui: &mut egui::Ui, rgba: &mut [u8; 4]) -> bool {
    let mut c = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
    let changed = egui::color_picker::color_edit_button_srgba(
        ui,
        &mut c,
        egui::color_picker::Alpha::OnlyBlend,
    )
    .changed();
    if changed {
        *rgba = c.to_srgba_unmultiplied();
    }
    changed
}

/// sRGB color button for `[u8; 3]`.
pub fn color_rgb(ui: &mut egui::Ui, rgb: &mut [u8; 3]) -> bool {
    let mut c = [rgb[0], rgb[1], rgb[2]];
    let changed = ui.color_edit_button_srgb(&mut c).changed();
    if changed {
        *rgb = c;
    }
    changed
}

/// Lerp two colors (animations).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}
