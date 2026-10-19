//! Drawing a laid-out screen with the 2D calls every backend has. Nothing
//! here is per target: the N64 gets the same fills, sprites and glyphs as
//! the PC (its text as glyph runs, its rects as fills).

use core::fmt::Write as _;

use super::layout::{LABEL_LEN, TRACK_MIN, fit_prefix, floor, image_rect, round, wrap};
use super::node::{NodeKind, Overflow, VAlign};
use super::values::{UiValue, UiValues};
use super::{Slot, Ui, UiEnv, UiNode, UiScreen, flags};
use crate::math::{Color, Rect, Vec2, Vec3};
use crate::math3d;
use crate::render::{
    Material, ModelParams, Renderer, Screen, SpriteParams, TextAlign, TextParams, Transform3,
};
use crate::render3d::{Camera3, NEAR_PLANE};
use crate::text::TextBuf;

/// The text a node shows: its key in the current language (else its own
/// text, else the key), with the bound value put in for `{}` (or after it).
pub(super) fn label<'b, const V: usize>(
    screen: &UiScreen<'b>,
    values: &UiValues<V>,
    env: &'b UiEnv<'_>,
    n: &UiNode<'b>,
    item: u16,
    buf: &'b mut TextBuf<LABEL_LEN>,
) -> &'b str {
    let _ = screen;
    let template = if n.key.is_empty() {
        n.text
    } else {
        match env.strings.get(n.key) {
            Some(t) => t,
            None if !n.text.is_empty() => n.text,
            None => n.key,
        }
    };
    let bound = !n.bind.is_empty()
        && !matches!(
            n.kind,
            NodeKind::Toggle | NodeKind::List { .. } | NodeKind::Image { .. }
        );
    if !bound {
        return template;
    }
    buf.clear();
    let (before, after) = match template.find("{}") {
        Some(at) => (&template[..at], &template[at + 2..]),
        None => (template, ""),
    };
    let _ = buf.write_str(before);
    match values.get(n.bind, item) {
        Some(UiValue::Int(v)) => {
            let _ = write!(buf, "{v}");
        }
        Some(UiValue::Bool(b)) => {
            let _ = buf.write_str(if b { "On" } else { "Off" });
        }
        Some(UiValue::Text(t)) => {
            let _ = buf.write_str(t.as_str());
        }
        None => {}
    }
    let _ = buf.write_str(after);
    buf.as_str()
}

/// Most lines a wrapped text draws.
const MAX_LINES: usize = 16;

/// Colors and state of a control this frame.
struct Look {
    fill: Option<Color>,
    text: Color,
    focused: bool,
}

impl<const N: usize> Ui<N> {
    /// Lay out (when anything changed) and draw the whole screen: the main
    /// screen's nodes where the renderer is now, then — when the screen has
    /// bottom-screen nodes and the target a second screen — the bottom's,
    /// after [`Renderer::on_screen`]. Reports [`Renderer::ui_stats`].
    pub fn draw(&mut self, renderer: &mut dyn Renderer, screen: &UiScreen<'_>, env: &UiEnv<'_>) {
        self.prepare(renderer, screen, env);
        self.stats.draws = 0;
        self.draw_slots(renderer, screen, env, Screen::Top);
        if self.slots().iter().any(|s| s.on == Screen::Bottom) && renderer.on_screen(Screen::Bottom)
        {
            self.draw_slots(renderer, screen, env, Screen::Bottom);
        }
        renderer.ui_stats(self.stats);
    }

    /// Draw only the nodes on `which` (the renderer must already be on
    /// it): for games that draw their own content on the bottom screen
    /// between the two halves.
    pub fn draw_on(
        &mut self,
        renderer: &mut dyn Renderer,
        screen: &UiScreen<'_>,
        env: &UiEnv<'_>,
        which: Screen,
    ) {
        self.prepare(renderer, screen, env);
        if which == Screen::Top {
            self.stats.draws = 0;
        }
        self.draw_slots(renderer, screen, env, which);
        renderer.ui_stats(self.stats);
    }

    fn prepare(&mut self, renderer: &mut dyn Renderer, screen: &UiScreen<'_>, env: &UiEnv<'_>) {
        if self.needs_layout(screen, env) {
            let measure: &dyn Renderer = renderer;
            self.layout(screen, env, measure);
        }
    }

    fn draw_slots(
        &mut self,
        r: &mut dyn Renderer,
        screen: &UiScreen<'_>,
        env: &UiEnv<'_>,
        which: Screen,
    ) {
        let focus = self.focus_slot();
        let pressed = self.pressed;
        for i in 0..self.count {
            let slot = self.slots[i];
            if slot.on != which || self.hidden(i) {
                continue;
            }
            let n = screen.node(slot.node as usize);
            let rect = self.rects[i];
            let is_focus = focus == Some(i);
            let is_pressed = is_focus
                && (pressed == Some((slot.node, slot.item))
                    || (self.accept_down && n.kind.is_focusable()));
            let look = self.look(screen, &n, &slot, is_focus, is_pressed);
            let scale = self.frames[which.index() as usize].scale;
            match n.kind {
                NodeKind::Rect {
                    border,
                    border_color,
                } => {
                    self.fill(r, rect, n.color.unwrap_or(screen.theme.panel));
                    if border > 0 && border_color.a > 0 {
                        self.outline(r, rect, round(border as f32 * scale).max(1.0), border_color);
                    }
                }
                NodeKind::Text => {
                    let content = inset_px(rect, &n, scale);
                    let color = n.color.unwrap_or(screen.theme.text);
                    self.text(r, screen, env, &n, &slot, content, color, scale);
                }
                NodeKind::Button => {
                    if let Some(fill) = look.fill {
                        self.fill(r, rect, fill);
                    }
                    if look.focused {
                        self.focus_ring(r, rect, screen, scale);
                    }
                    let content = inset_px(rect, &n, scale);
                    self.text(r, screen, env, &n, &slot, content, look.text, scale);
                }
                NodeKind::Toggle => {
                    if let Some(fill) = look.fill {
                        self.fill(r, rect, fill);
                    }
                    if look.focused {
                        self.focus_ring(r, rect, screen, scale);
                    }
                    let content = inset_px(rect, &n, scale);
                    let lh = r
                        .measure_text(font_of(screen, &n), "", text_scale(screen, &n) * scale)
                        .y;
                    let side = lh.min(content.size.y).max(scale);
                    let bx = Rect::new(
                        content.pos.x,
                        floor(content.pos.y + (content.size.y - side) * 0.5),
                        side,
                        side,
                    );
                    self.fill(r, bx, look.text);
                    let s1 = scale.max(1.0);
                    self.fill(r, shrink(bx, s1), Color::rgb(16, 18, 26));
                    let on = self
                        .values
                        .get(n.bind, slot.item)
                        .is_some_and(|v| v.truthy());
                    if on {
                        self.fill(r, shrink(bx, 2.0 * s1), screen.theme.accent);
                    }
                    let gap = super::layout::INNER_GAP * scale;
                    let label = Rect::new(
                        bx.pos.x + side + gap,
                        content.pos.y,
                        (content.size.x - side - gap).max(0.0),
                        content.size.y,
                    );
                    self.text(r, screen, env, &n, &slot, label, look.text, scale);
                }
                NodeKind::Slider { min, max, .. } => {
                    if let Some(fill) = look.fill {
                        self.fill(r, rect, fill);
                    }
                    if look.focused {
                        self.focus_ring(r, rect, screen, scale);
                    }
                    let content = inset_px(rect, &n, scale);
                    let mut buf = TextBuf::<LABEL_LEN>::new();
                    let text = label(screen, &self.values, env, &n, slot.item, &mut buf);
                    let lw = r
                        .measure_text(font_of(screen, &n), text, text_scale(screen, &n) * scale)
                        .x;
                    let gap = if lw > 0.0 {
                        super::layout::INNER_GAP * scale
                    } else {
                        0.0
                    };
                    let room = Rect::new(content.pos.x, content.pos.y, lw, content.size.y);
                    self.text(r, screen, env, &n, &slot, room, look.text, scale);
                    let track = self
                        .track(slot.node, slot.item)
                        .unwrap_or_else(|| self.slider_track(rect, &n, lw + gap, scale));
                    let value = self
                        .values
                        .get(n.bind, slot.item)
                        .and_then(|v| v.int())
                        .unwrap_or(min);
                    let t = if max > min {
                        ((value - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let h = scale.max(1.0) * 2.0;
                    let y = floor(track.pos.y + (track.size.y - h) * 0.5);
                    self.fill(
                        r,
                        Rect::new(track.pos.x, y, track.size.x, h),
                        screen.theme.dim,
                    );
                    let filled = floor(track.size.x * t);
                    self.fill(r, Rect::new(track.pos.x, y, filled, h), look.text);
                    let kw = scale.max(1.0) * 3.0;
                    let kh = (track.size.y * 0.8).max(h);
                    let kx = floor(track.pos.x + filled - kw * 0.5)
                        .clamp(track.pos.x, track.pos.x + track.size.x - kw);
                    let ky = floor(track.pos.y + (track.size.y - kh) * 0.5);
                    self.fill(r, Rect::new(kx, ky, kw, kh), screen.theme.accent);
                }
                NodeKind::Image {
                    sprite,
                    src,
                    nine,
                    fit,
                } => {
                    let tint = n.color.unwrap_or(Color::WHITE);
                    if nine != [0; 4] {
                        self.nine_slice(r, sprite, src, nine, rect, scale, tint);
                    } else {
                        let at = image_rect(rect, src, fit, scale);
                        let (sw, sh) = (src.0.max(1) as f32, src.1.max(1) as f32);
                        r.draw_sprite(
                            sprite,
                            at.pos,
                            &SpriteParams {
                                scale: Vec2::new(at.size.x / sw, at.size.y / sh),
                                tint,
                                ..SpriteParams::DEFAULT
                            },
                        );
                        self.stats.draws = self.stats.draws.saturating_add(1);
                    }
                }
                NodeKind::Model3d {
                    model,
                    yaw,
                    pitch,
                    spin,
                    extent,
                    lift,
                } => {
                    if let Some(c) = n.color {
                        self.fill(r, rect, c);
                    }
                    let size = r
                        .screen_size(which)
                        .unwrap_or(Vec2::new(rect.size.x, rect.size.y));
                    model_in_box(
                        r,
                        model,
                        rect,
                        size,
                        (yaw as f32 + spin as f32 * self.time, pitch as f32),
                        extent,
                        lift,
                    );
                }
                NodeKind::Overlay
                | NodeKind::Stack { .. }
                | NodeKind::Grid { .. }
                | NodeKind::Scroll { .. }
                | NodeKind::List { .. } => {
                    if let Some(c) = n.color {
                        self.fill(r, rect, c);
                    }
                }
            }
        }
    }

    fn look(
        &self,
        screen: &UiScreen<'_>,
        n: &UiNode<'_>,
        slot: &Slot,
        focused: bool,
        pressed: bool,
    ) -> Look {
        let t = &screen.theme;
        let disabled = slot.has(flags::DISABLED);
        let base = match n.kind {
            NodeKind::Button => Some(n.color.unwrap_or(t.button)),
            _ => n.color,
        };
        if disabled {
            return Look {
                fill: base,
                text: t.dim,
                focused: false,
            };
        }
        if pressed {
            return Look {
                fill: Some(t.pressed),
                text: t.focus_text,
                focused,
            };
        }
        if focused {
            return Look {
                fill: Some(t.focus),
                text: t.focus_text,
                focused,
            };
        }
        Look {
            fill: base,
            text: t.button_text,
            focused,
        }
    }

    /// Where a slider's track is (pixels), after `label_w` of label.
    pub(super) fn slider_track(
        &self,
        rect: Rect,
        n: &UiNode<'_>,
        label_w: f32,
        scale: f32,
    ) -> Rect {
        let content = inset_px(rect, n, scale);
        let x = content.pos.x + label_w;
        let w = (content.size.x - label_w).max(TRACK_MIN.min(content.size.x));
        Rect::new(x, content.pos.y, w, content.size.y)
    }

    fn fill(&mut self, r: &mut dyn Renderer, rect: Rect, color: Color) {
        if color.a == 0 || rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        r.fill_rect(rect, color);
        self.stats.draws = self.stats.draws.saturating_add(1);
    }

    fn outline(&mut self, r: &mut dyn Renderer, rect: Rect, w: f32, color: Color) {
        let (x, y, rw, rh) = (rect.pos.x, rect.pos.y, rect.size.x, rect.size.y);
        let w = w.min(rw * 0.5).min(rh * 0.5);
        self.fill(r, Rect::new(x, y, rw, w), color);
        self.fill(r, Rect::new(x, y + rh - w, rw, w), color);
        self.fill(r, Rect::new(x, y + w, w, rh - 2.0 * w), color);
        self.fill(r, Rect::new(x + rw - w, y + w, w, rh - 2.0 * w), color);
    }

    fn focus_ring(&mut self, r: &mut dyn Renderer, rect: Rect, screen: &UiScreen<'_>, scale: f32) {
        self.outline(r, rect, round(scale).max(1.0), screen.theme.accent);
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        r: &mut dyn Renderer,
        screen: &UiScreen<'_>,
        env: &UiEnv<'_>,
        n: &UiNode<'_>,
        slot: &Slot,
        content: Rect,
        color: Color,
        scale: f32,
    ) {
        let mut buf = TextBuf::<LABEL_LEN>::new();
        let text = label(screen, &self.values, env, n, slot.item, &mut buf);
        if text.is_empty() {
            return;
        }
        let font = font_of(screen, n);
        let px = text_scale(screen, n) * scale;
        let measure: &dyn Renderer = r;
        let width = |s: &str| measure.measure_text(font, s, px).x;
        let lh = measure.measure_text(font, "", px).y;
        let params = TextParams {
            scale: px,
            tint: color,
            align: n.align,
        };
        let x = match n.align {
            TextAlign::Left => content.pos.x,
            TextAlign::Center => content.pos.x + content.size.x * 0.5,
            TextAlign::Right => content.pos.x + content.size.x,
        };
        // One line, cut as the node says.
        let mut one = TextBuf::<LABEL_LEN>::new();
        let single = match n.overflow {
            Overflow::Wrap => None,
            Overflow::Visible => Some(text),
            Overflow::Clip => Some(fit_prefix(text, content.size.x, &width)),
            Overflow::Ellipsis => {
                if width(text) <= content.size.x + 0.5 {
                    Some(text)
                } else {
                    let head = fit_prefix(text, (content.size.x - width("...")).max(0.0), &width);
                    let _ = one.write_str(head.trim_end());
                    let _ = one.write_str("...");
                    Some(one.as_str())
                }
            }
        };
        // Wrapped lines as byte ranges of `text` (measured before drawing).
        let mut lines = [(0u16, 0u16); MAX_LINES];
        let mut count = 0;
        if single.is_none() {
            let base = text.as_ptr() as usize;
            wrap(text, content.size.x, &width, &mut |line| {
                if count < MAX_LINES {
                    let at = line.as_ptr() as usize - base;
                    lines[count] = (at as u16, (at + line.len()) as u16);
                    count += 1;
                }
            });
        }
        let block_h = match single {
            Some(s) => measure.measure_text(font, s, px).y.max(lh),
            None => count.max(1) as f32 * lh,
        };
        let y = match n.valign {
            VAlign::Top => content.pos.y,
            VAlign::Middle => content.pos.y + (content.size.y - block_h) * 0.5,
            VAlign::Bottom => content.pos.y + content.size.y - block_h,
        };
        let y = floor(y);
        let mut glyphs = 0u16;
        match single {
            Some(s) => {
                r.draw_text(font, Vec2::new(x, y), s, &params);
                glyphs = glyph_count(s);
            }
            None => {
                for (k, &(a, b)) in lines[..count].iter().enumerate() {
                    let line = &text[a as usize..b as usize];
                    r.draw_text(font, Vec2::new(x, y + k as f32 * lh), line, &params);
                    glyphs = glyphs.saturating_add(glyph_count(line));
                }
            }
        }
        self.stats.draws = self.stats.draws.saturating_add(glyphs);
    }

    #[allow(clippy::too_many_arguments)]
    fn nine_slice(
        &mut self,
        r: &mut dyn Renderer,
        sprite: crate::render::SpriteId,
        src: (u16, u16),
        nine: [u8; 4],
        rect: Rect,
        scale: f32,
        tint: Color,
    ) {
        let (sw, sh) = (src.0 as f32, src.1 as f32);
        let [l, t, rr, b] = nine.map(|v| v as f32);
        // Source columns and rows, then where they go (corners at the
        // layout's scale, edges and the middle stretched).
        let sx = [0.0, l, sw - rr, sw];
        let sy = [0.0, t, sh - b, sh];
        let (dl, dt) = (round(l * scale), round(t * scale));
        let (dr, db) = (round(rr * scale), round(b * scale));
        let dx = [
            rect.pos.x,
            rect.pos.x + dl,
            rect.pos.x + rect.size.x - dr,
            rect.pos.x + rect.size.x,
        ];
        let dy = [
            rect.pos.y,
            rect.pos.y + dt,
            rect.pos.y + rect.size.y - db,
            rect.pos.y + rect.size.y,
        ];
        for row in 0..3 {
            for col in 0..3 {
                let (w, h) = (sx[col + 1] - sx[col], sy[row + 1] - sy[row]);
                let (dw, dh) = (dx[col + 1] - dx[col], dy[row + 1] - dy[row]);
                if w <= 0.0 || h <= 0.0 || dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                r.draw_sprite(
                    sprite,
                    Vec2::new(dx[col], dy[row]),
                    &SpriteParams {
                        scale: Vec2::new(dw / w, dh / h),
                        tint,
                        region: Some(Rect::new(sx[col], sy[row], w, h)),
                        ..SpriteParams::DEFAULT
                    },
                );
                self.stats.draws = self.stats.draws.saturating_add(1);
            }
        }
    }
}

fn glyph_count(s: &str) -> u16 {
    s.chars().filter(|c| !c.is_whitespace()).count() as u16
}

fn shrink(r: Rect, by: f32) -> Rect {
    Rect::new(
        r.pos.x + by,
        r.pos.y + by,
        (r.size.x - 2.0 * by).max(0.0),
        (r.size.y - 2.0 * by).max(0.0),
    )
}

/// A slot's content box in pixels (padding is in virtual units).
fn inset_px(rect: Rect, n: &UiNode<'_>, scale: f32) -> Rect {
    let [l, t, r, b] = n.padding.map(|v| round(v as f32 * scale));
    Rect::new(
        rect.pos.x + l,
        rect.pos.y + t,
        (rect.size.x - l - r).max(0.0),
        (rect.size.y - t - b).max(0.0),
    )
}

fn font_of(screen: &UiScreen<'_>, n: &UiNode<'_>) -> crate::render::FontId {
    n.font.unwrap_or(screen.theme.font)
}

fn text_scale(screen: &UiScreen<'_>, n: &UiNode<'_>) -> f32 {
    if n.text_scale > 0 {
        n.text_scale as f32
    } else {
        screen.theme.text_scale.max(1) as f32
    }
}

/// Draw a 3D model so it fills `rect` (pixels of a `screen`-sized
/// picture): a camera at the origin looking down +z, the model just past the
/// near plane at the box's place, scaled so `extent` / 100 model units fill
/// the box's height. Nothing in the world is that close, so it is drawn in
/// front of the scene on every target's z-buffer.
pub(super) fn model_in_box(
    r: &mut dyn Renderer,
    model: crate::render::ModelId,
    rect: Rect,
    screen: Vec2,
    angles: (f32, f32),
    extent: u16,
    lift: i16,
) {
    let camera = Camera3 {
        eye: Vec3::ZERO,
        target: Vec3::new(0.0, 0.0, 1.0),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let half = camera.fov_y * 0.5;
    let focal = math3d::cos(half) / math3d::sin(half) * screen.y * 0.5;
    let depth = NEAR_PLANE * 4.0;
    let cx = rect.pos.x + rect.size.x * 0.5;
    let cy = rect.pos.y + rect.size.y * 0.5;
    let at = Vec3::new(
        (cx - screen.x * 0.5) * depth / focal,
        (screen.y * 0.5 - cy) * depth / focal,
        depth,
    );
    let world_h = rect.size.y * depth / focal;
    let s = world_h / (extent.max(1) as f32 / 100.0);
    let deg = core::f32::consts::PI / 180.0;
    r.set_camera(&camera);
    r.draw_model(
        model,
        &Transform3 {
            position: Vec3::new(at.x, at.y - lift as f32 / 100.0 * s, at.z),
            rotation: Vec3::new(angles.1 * deg, angles.0 * deg, 0.0),
            scale: Vec3::new(s, s, s),
        },
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
}
