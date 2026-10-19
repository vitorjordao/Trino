//! The layout solver: virtual units in, pixel rects out (one per slot).
//!
//! A parent decides each child's rect: an overlay by the child's anchor and
//! offset, a stack one after the other, a grid by cells. A `Fit` size asks
//! the node's content (text, sprite, children) first. All of it is plain
//! `f32` arithmetic in a fixed order, so every target lays a screen out
//! identically; pixels come from rounding both edges, so neighbors never
//! leave gaps.

use super::node::{Align, AxisAnchor, Dir, ImageFit, Justify, NodeKind, Overflow, Scaling, Size};
use super::values::{NO_ITEM, lookup};
use super::{
    Frame, Insets, MAX_DEPTH, MAX_SCROLLS, NONE, ScrollBox, Slot, TextMeasure, Ui, UiEnv, UiNode,
    UiScreen, UiStats, flags,
};
use crate::math::{Rect, Vec2};
use crate::render::{FontId, Screen};
use crate::text::TextBuf;

/// Room a slider's track takes at least, virtual units.
pub(super) const TRACK_MIN: f32 = 48.0;
/// Between a toggle's box or a slider's label and the rest.
pub(super) const INNER_GAP: f32 = 4.0;
/// Longest label, bytes (a bound value substituted in).
pub(super) const LABEL_LEN: usize = 96;

pub(super) fn floor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if t > v { t - 1.0 } else { t }
}

pub(super) fn round(v: f32) -> f32 {
    floor(v + 0.5)
}

fn axis(v: Vec2, main: Dir) -> (f32, f32) {
    match main {
        Dir::Horizontal => (v.x, v.y),
        Dir::Vertical => (v.y, v.x),
    }
}

fn from_axis(m: f32, c: f32, main: Dir) -> Vec2 {
    match main {
        Dir::Horizontal => Vec2::new(m, c),
        Dir::Vertical => Vec2::new(c, m),
    }
}

/// `r` shrunk by `pad` (left, top, right, bottom), never negative.
pub(super) fn inset(r: Rect, pad: [u8; 4]) -> Rect {
    let (l, t, rr, b) = (pad[0] as f32, pad[1] as f32, pad[2] as f32, pad[3] as f32);
    Rect::new(
        r.pos.x + l,
        r.pos.y + t,
        (r.size.x - l - rr).max(0.0),
        (r.size.y - t - b).max(0.0),
    )
}

fn inside(outer: &Rect, r: &Rect) -> bool {
    const E: f32 = 0.01;
    r.pos.x >= outer.pos.x - E
        && r.pos.y >= outer.pos.y - E
        && r.pos.x + r.size.x <= outer.pos.x + outer.size.x + E
        && r.pos.y + r.size.y <= outer.pos.y + outer.size.y + E
}

/// A frame for a screen of `px` pixels.
pub(super) fn frame_for(
    px: Vec2,
    safe: Insets,
    reference: (u16, u16),
    scaling: Scaling,
    interface: f32,
) -> Frame {
    let fit = (px.x / reference.0 as f32).min(px.y / reference.1 as f32) * interface;
    let scale = match scaling {
        Scaling::Integer if fit >= 1.0 => floor(fit),
        _ => fit,
    }
    .max(1.0 / 64.0);
    let size = Vec2::new(px.x / scale, px.y / scale);
    let (l, t) = (safe.left as f32 / scale, safe.top as f32 / scale);
    let (r, b) = (safe.right as f32 / scale, safe.bottom as f32 / scale);
    Frame {
        scale,
        size,
        safe: Rect::new(l, t, (size.x - l - r).max(0.0), (size.y - t - b).max(0.0)),
        present: true,
    }
}

/// Greedy word wrap of `text` at `max` wide: each line to `emit`. A word
/// wider than `max` gets a line of its own (and overflows).
pub(super) fn wrap(text: &str, max: f32, width: &dyn Fn(&str) -> f32, emit: &mut dyn FnMut(&str)) {
    for para in text.split('\n') {
        let mut start = 0;
        let mut fit_end: Option<usize> = None;
        let mut at = 0;
        loop {
            let end = para[at..].find(' ').map_or(para.len(), |k| at + k);
            if fit_end.is_none() || width(&para[start..end]) <= max {
                fit_end = Some(end);
                if end >= para.len() {
                    emit(&para[start..]);
                    break;
                }
                at = end + 1;
            } else {
                let cut = fit_end.unwrap_or(end);
                emit(&para[start..cut]);
                start = cut;
                while para[start..].starts_with(' ') {
                    start += 1;
                }
                at = start;
                fit_end = None;
                if start >= para.len() {
                    break;
                }
            }
        }
    }
}

/// The longest prefix of `text` (whole characters) at most `max` wide.
pub(super) fn fit_prefix<'t>(text: &'t str, max: f32, width: &dyn Fn(&str) -> f32) -> &'t str {
    let mut best = 0;
    for (i, ch) in text.char_indices() {
        let end = i + ch.len_utf8();
        if width(&text[..end]) > max {
            break;
        }
        best = end;
    }
    &text[..best]
}

impl<const N: usize> Ui<N> {
    /// Lay `screen` out for `env`, measuring text with `measure`. [`draw`]
    /// does it on its own when anything changed; call it to read rects
    /// before the first draw (tools, tests).
    ///
    /// [`draw`]: Ui::draw
    pub fn layout<M: TextMeasure + ?Sized>(
        &mut self,
        screen: &UiScreen<'_>,
        env: &UiEnv<'_>,
        measure: &M,
    ) {
        let name = crate::asset::asset_id(screen.name);
        if self.name != Some(name) {
            // Another screen: start over (a hot-reloaded copy of the same
            // screen keeps its focus).
            self.reset();
            self.name = Some(name);
        }
        self.frames[0] = frame_for(
            env.top,
            env.safe[0],
            screen.reference,
            screen.scaling,
            env.interface_scale,
        );
        self.frames[1] = match env.bottom {
            Some(px) => frame_for(
                px,
                env.safe[1],
                screen.reference,
                screen.scaling,
                env.interface_scale,
            ),
            None => Frame::NONE,
        };
        self.text_mul = env.access.text_mul(screen.scaling, self.frames[0].scale);
        self.count = 0;
        self.stats = UiStats::default();
        for s in self.scroll.iter_mut() {
            s.max = 0.0;
        }
        self.tracks = [(NONE, 0, Rect::new(0.0, 0.0, 0.0, 0.0)); super::MAX_SLIDERS];
        if !screen.is_empty() {
            let mut solver = Solver {
                ui: self,
                screen,
                env,
                measure,
            };
            let root = screen.node(0);
            if solver.visible(&root, NO_ITEM) {
                let on = solver.screen_of(&root, Screen::Top);
                let frame = solver.ui.frames[on.index() as usize];
                let rect = if root.bleed {
                    Rect::new(0.0, 0.0, frame.size.x, frame.size.y)
                } else {
                    frame.safe
                };
                solver.place(0, NO_ITEM, NONE, rect, on, 0, None);
            }
        }
        // Scroll offsets stay within what their content allows.
        for s in self.scroll.iter_mut() {
            s.offset = s.offset.clamp(0.0, s.max);
        }
        self.stats.nodes = self.count as u16;
        self.laid_out = true;
        self.key = self.layout_key(screen, env);
        self.last_env = Some(super::EnvHint::of(env));
    }

    /// What a layout depends on, to skip one that would change nothing.
    pub(super) fn layout_key(&self, screen: &UiScreen<'_>, env: &UiEnv<'_>) -> [u32; 8] {
        let data = match screen.nodes {
            super::Nodes::Slice(s) => s.as_ptr() as usize as u32,
            super::Nodes::Baked(_) => screen.name.as_ptr() as usize as u32,
        };
        let scroll = self.scroll.iter().fold(0u32, |h, s| {
            h.rotate_left(5) ^ s.offset.to_bits() ^ s.node as u32
        });
        [
            data ^ (screen.len() as u32).rotate_left(16),
            env.top.x.to_bits() ^ env.top.y.to_bits().rotate_left(7),
            env.bottom
                .map_or(0, |b| b.x.to_bits() ^ b.y.to_bits().rotate_left(7)),
            env.family as u32
                ^ env.interface_scale.to_bits().rotate_left(3)
                ^ env.access.text_scale.to_bits().rotate_left(13)
                ^ (env.access.high_contrast as u32) << 31,
            env.strings.revision(),
            self.values.revision() ^ env.data.revision().unwrap_or(0).rotate_left(11),
            scroll,
            (env.safe[0].left as u32) << 24
                ^ (env.safe[0].top as u32) << 16
                ^ (env.safe[0].right as u32) << 8
                ^ env.safe[0].bottom as u32,
        ]
    }

    /// Whether [`draw`](Ui::draw) must lay out again.
    pub(super) fn needs_layout(&self, screen: &UiScreen<'_>, env: &UiEnv<'_>) -> bool {
        !self.laid_out
            || env.data.revision().is_none()
            || self.name != Some(crate::asset::asset_id(screen.name))
            || self.layout_key(screen, env) != self.key
    }
}

struct Solver<'s, 'a, 'e, M: ?Sized, const N: usize> {
    ui: &'s mut Ui<N>,
    screen: &'s UiScreen<'a>,
    env: &'s UiEnv<'e>,
    measure: &'s M,
}

impl<M: TextMeasure + ?Sized, const N: usize> Solver<'_, '_, '_, M, N> {
    fn visible(&self, n: &UiNode<'_>, item: u16) -> bool {
        n.visible_on.contains(self.env.family)
            && (n.visible_if.is_empty()
                || lookup(&self.ui.values, self.env.data, n.visible_if, item)
                    .is_some_and(|v| v.truthy()))
    }

    /// The screen a node goes on, given its parent's.
    fn screen_of(&self, n: &UiNode<'_>, parent_on: Screen) -> Screen {
        if n.screen == Screen::Bottom && self.ui.frames[1].present {
            Screen::Bottom
        } else if n.screen == Screen::Top {
            Screen::Top
        } else {
            parent_on
        }
    }

    fn font(&self, n: &UiNode<'_>) -> (FontId, f32) {
        let scale = if n.text_scale > 0 {
            n.text_scale
        } else {
            self.screen.theme.text_scale.max(1)
        };
        (
            super::draw::font_of(self.screen, self.env, n),
            scale as f32 * self.ui.text_mul,
        )
    }

    fn line_height(&self, n: &UiNode<'_>) -> f32 {
        let (font, scale) = self.font(n);
        self.measure.text_size(font, "", scale).y
    }

    fn text_width(&self, n: &UiNode<'_>, s: &str) -> f32 {
        let (font, scale) = self.font(n);
        self.measure.text_size(font, s, scale).x
    }

    /// The node's label, its size, and whether it wraps past `max_w`.
    fn label_size(&self, n: &UiNode<'_>, item: u16, max_w: f32) -> Vec2 {
        let mut buf = TextBuf::<LABEL_LEN>::new();
        let text = super::draw::label(self.screen, &self.ui.values, self.env, n, item, &mut buf);
        let lh = self.line_height(n);
        if n.overflow == Overflow::Wrap {
            let (mut w, mut lines) = (0.0f32, 0u32);
            wrap(text, max_w, &|s| self.text_width(n, s), &mut |line| {
                w = w.max(self.text_width(n, line));
                lines += 1;
            });
            Vec2::new(w, lines.max(1) as f32 * lh)
        } else {
            let (font, scale) = self.font(n);
            let s = self.measure.text_size(font, text, scale);
            Vec2::new(s.x, s.y.max(lh))
        }
    }

    fn list_count(&self, n: &UiNode<'_>, item: u16) -> usize {
        let NodeKind::List { max, .. } = n.kind else {
            return 0;
        };
        let want = lookup(&self.ui.values, self.env.data, n.bind, item)
            .and_then(|v| v.int())
            .unwrap_or(0)
            .max(0) as usize;
        let cap = if max > 0 { max as usize } else { 256 };
        want.min(cap)
    }

    /// The visible children of `i` as seen by list item `item`.
    fn kids(&self, i: usize, item: u16) -> impl Iterator<Item = usize> + '_ {
        self.screen
            .children(i)
            .filter(move |&c| self.visible(&self.screen.node(c), item))
    }

    /// The node's outer size when its parent offers `avail`.
    fn outer(&self, i: usize, item: u16, avail: Vec2, depth: usize) -> Vec2 {
        let n = self.screen.node(i);
        let pad = Vec2::new(
            (n.padding[0] + n.padding[2]) as f32,
            (n.padding[1] + n.padding[3]) as f32,
        );
        let clamp = |v: f32, min: u16, max: u16| {
            let v = v.max(min as f32);
            if max > 0 { v.min(max as f32) } else { v }
        };
        let w = match n.size.0 {
            Size::Px(v) => v as f32,
            Size::Percent(p) => avail.x * p as f32 / 100.0,
            Size::Fill => avail.x,
            Size::Fit => {
                self.content(i, item, Vec2::new(avail.x - pad.x, avail.y - pad.y), depth)
                    .x
                    + pad.x
            }
        };
        let w = clamp(w, n.min.0, n.max.0);
        let h = match n.size.1 {
            Size::Px(v) => v as f32,
            Size::Percent(p) => avail.y * p as f32 / 100.0,
            Size::Fill => avail.y,
            Size::Fit => {
                self.content(i, item, Vec2::new(w - pad.x, avail.y - pad.y), depth)
                    .y
                    + pad.y
            }
        };
        Vec2::new(w, clamp(h, n.min.1, n.max.1))
    }

    /// The node's content size (inside its padding) for `avail` room.
    fn content(&self, i: usize, item: u16, avail: Vec2, depth: usize) -> Vec2 {
        if depth >= MAX_DEPTH {
            return Vec2::ZERO;
        }
        let n = self.screen.node(i);
        let avail = Vec2::new(avail.x.max(0.0), avail.y.max(0.0));
        let own = match n.kind {
            NodeKind::Text | NodeKind::Button => self.label_size(&n, item, avail.x),
            NodeKind::Toggle => {
                let lh = self.line_height(&n);
                let l = self.label_size(&n, item, avail.x - lh - INNER_GAP);
                Vec2::new(lh + INNER_GAP + l.x, l.y.max(lh))
            }
            NodeKind::Slider { .. } => {
                let lh = self.line_height(&n);
                let l = self.label_size(&n, item, avail.x);
                let gap = if l.x > 0.0 { INNER_GAP } else { 0.0 };
                Vec2::new(l.x + gap + TRACK_MIN, l.y.max(lh))
            }
            NodeKind::Image { src, .. } => Vec2::new(src.0.max(1) as f32, src.1.max(1) as f32),
            NodeKind::Model3d { .. } => Vec2::new(48.0, 48.0),
            NodeKind::Stack { dir, gap, .. } | NodeKind::Scroll { dir, gap } => {
                let (mut main, mut cross, mut k) = (0.0f32, 0.0f32, 0);
                for c in self.kids(i, item) {
                    let s = self.outer(c, item, avail, depth + 1);
                    let (m, x) = axis(s, dir);
                    if axis_size(&self.screen.node(c), dir) != Size::Fill {
                        main += m;
                    }
                    cross = cross.max(x);
                    k += 1;
                }
                if k > 1 {
                    main += gap as f32 * (k - 1) as f32;
                }
                from_axis(main, cross, dir)
            }
            NodeKind::List { dir, gap, .. } => {
                let (mut main, mut cross) = (0.0f32, 0.0f32);
                let count = self.list_count(&n, item);
                if let Some(t) = self.kids(i, item).next() {
                    for k in 0..count {
                        let s = self.outer(t, k as u16, avail, depth + 1);
                        let (m, x) = axis(s, dir);
                        main += m;
                        cross = cross.max(x);
                    }
                    if count > 1 {
                        main += gap as f32 * (count - 1) as f32;
                    }
                }
                from_axis(main, cross, dir)
            }
            NodeKind::Grid { cols, gap } => {
                let cols = cols.clamp(1, 32) as usize;
                let (mut colw, mut total_h, mut row_h, mut k) = (0.0f32, 0.0f32, 0.0f32, 0);
                for c in self.kids(i, item) {
                    let s = self.outer(c, item, avail, depth + 1);
                    colw = colw.max(s.x);
                    row_h = row_h.max(s.y);
                    k += 1;
                    if k % cols == 0 {
                        total_h += row_h + gap.1 as f32;
                        row_h = 0.0;
                    }
                }
                if k % cols != 0 {
                    total_h += row_h + gap.1 as f32;
                }
                if k > 0 {
                    total_h -= gap.1 as f32;
                }
                let used = cols.min(k.max(1));
                Vec2::new(
                    colw * used as f32 + gap.0 as f32 * (used - 1) as f32,
                    total_h,
                )
            }
            NodeKind::Overlay | NodeKind::Rect { .. } => Vec2::ZERO,
        };
        if matches!(
            n.kind,
            NodeKind::Stack { .. }
                | NodeKind::Scroll { .. }
                | NodeKind::List { .. }
                | NodeKind::Grid { .. }
        ) {
            return own;
        }
        // Anything else lays its children over each other.
        let mut all = own;
        for c in self.kids(i, item) {
            let cn = self.screen.node(c);
            let s = self.outer(c, item, avail, depth + 1);
            let (ax, ay) = cn.anchor.axes();
            if ax != AxisAnchor::Stretch {
                all.x = all.x.max(s.x + abs(cn.offset.0 as f32));
            }
            if ay != AxisAnchor::Stretch {
                all.y = all.y.max(s.y + abs(cn.offset.1 as f32));
            }
        }
        all
    }

    fn alloc(&mut self, node: usize, item: u16, parent: u16, on: Screen) -> Option<usize> {
        if self.ui.count >= N {
            self.ui.stats.dropped = self.ui.stats.dropped.saturating_add(1);
            return None;
        }
        let i = self.ui.count;
        self.ui.count += 1;
        self.ui.slots[i] = Slot {
            node: node as u16,
            item,
            parent,
            on,
            flags: 0,
        };
        Some(i)
    }

    fn to_px(&self, r: Rect, on: Screen) -> Rect {
        let s = self.ui.frames[on.index() as usize].scale;
        let x0 = round(r.pos.x * s);
        let y0 = round(r.pos.y * s);
        let x1 = round((r.pos.x + r.size.x) * s);
        let y1 = round((r.pos.y + r.size.y) * s);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Give node `i` the rect `rect` (virtual units of screen `on`) and lay
    /// its children out inside it. `clip` is the scroll box it must fit in
    /// to be drawn.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        i: usize,
        item: u16,
        parent: u16,
        rect: Rect,
        on: Screen,
        depth: usize,
        clip: Option<Rect>,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Some(slot) = self.alloc(i, item, parent, on) else {
            return;
        };
        let n = self.screen.node(i);
        let mut f = 0u8;
        // A node a scroll box cuts is not drawn, nor what it holds: a row
        // shows whole or not at all. A list (or a nested scroll box)
        // without a fill is only the rows' frame, longer than the box by
        // design: its rows decide one by one (issue #95).
        let frame =
            matches!(n.kind, NodeKind::Scroll { .. } | NodeKind::List { .. }) && n.color.is_none();
        if !frame && clip.is_some_and(|c| !inside(&c, &rect)) {
            f |= flags::CLIPPED;
        }
        if n.kind.is_focusable() {
            let enabled = n.enabled_if.is_empty()
                || lookup(&self.ui.values, self.env.data, n.enabled_if, item)
                    .is_some_and(|v| v.truthy());
            f |= if enabled {
                flags::FOCUSABLE
            } else {
                flags::DISABLED
            };
        }
        let content = super::layout::inset(rect, n.padding);
        if n.kind.has_text() {
            f |= self.text_fit(&n, item, content);
            if f & flags::OVERFLOW != 0 {
                self.ui.stats.overflow = self.ui.stats.overflow.saturating_add(1);
            }
        }
        self.ui.slots[slot].flags = f;
        self.ui.rects[slot] = self.to_px(rect, on);
        if let NodeKind::Slider { .. } = n.kind
            && let Some(k) = self.ui.tracks.iter().position(|t| t.0 == NONE)
        {
            let l = self.label_size(&n, item, content.size.x);
            let gap = if l.x > 0.0 { INNER_GAP } else { 0.0 };
            let x = content.pos.x + l.x + gap;
            let w = (content.size.x - l.x - gap).max(TRACK_MIN.min(content.size.x));
            let px = self.to_px(Rect::new(x, content.pos.y, w, content.size.y), on);
            self.ui.tracks[k] = (i as u16, item, px);
        }
        let slot = slot as u16;
        match n.kind {
            NodeKind::Stack {
                dir,
                gap,
                align,
                justify,
            } => {
                self.stack(
                    i, item, slot, content, on, depth, clip, dir, gap, align, justify,
                );
            }
            NodeKind::Scroll { dir, gap } => {
                let at = self.scroll_slot(i as u16, dir == Dir::Vertical);
                let offset = at.map_or(0.0, |k| self.ui.scroll[k].offset);
                let (vm, _) = axis(content.size, dir);
                let shifted = match dir {
                    Dir::Horizontal => Rect::new(
                        content.pos.x - offset,
                        content.pos.y,
                        content.size.x,
                        content.size.y,
                    ),
                    Dir::Vertical => Rect::new(
                        content.pos.x,
                        content.pos.y - offset,
                        content.size.x,
                        content.size.y,
                    ),
                };
                let inner = clip.map_or(content, |c| intersect(&c, &content));
                let used = self.stack(
                    i,
                    item,
                    slot,
                    shifted,
                    on,
                    depth,
                    Some(inner),
                    dir,
                    gap,
                    Align::Stretch,
                    Justify::Start,
                );
                if let Some(k) = at {
                    self.ui.scroll[k].max = (used - vm).max(0.0);
                }
            }
            NodeKind::List { dir, gap, .. } => {
                let count = self.list_count(&n, item);
                let Some(t) = self.kids(i, item).next() else {
                    return;
                };
                let (mut pos, cross) = (axis(content.pos, dir).0, axis(content.size, dir).1);
                for k in 0..count {
                    let s = self.outer(t, k as u16, content.size, depth + 1);
                    let (m, _) = axis(s, dir);
                    let r = match dir {
                        Dir::Horizontal => Rect::new(pos, content.pos.y, m, cross),
                        Dir::Vertical => Rect::new(content.pos.x, pos, cross, m),
                    };
                    self.place(t, k as u16, slot, r, on, depth + 1, clip);
                    pos += m + gap as f32;
                }
            }
            NodeKind::Grid { cols, gap } => {
                self.grid(i, item, slot, content, on, depth, clip, cols, gap);
            }
            _ => {
                let screen = self.screen;
                for c in screen.children(i) {
                    let cn = self.screen.node(c);
                    if !self.visible(&cn, item) {
                        continue;
                    }
                    let c_on = self.screen_of(&cn, on);
                    let room = if c_on != on || cn.bleed {
                        let fr = self.ui.frames[c_on.index() as usize];
                        if cn.bleed {
                            Rect::new(0.0, 0.0, fr.size.x, fr.size.y)
                        } else {
                            fr.safe
                        }
                    } else {
                        content
                    };
                    let r = self.anchored(c, item, room, depth);
                    self.place(c, item, slot, r, c_on, depth + 1, clip);
                }
            }
        }
    }

    /// A child's rect inside `room` by its anchor, offset and size.
    fn anchored(&self, c: usize, item: u16, room: Rect, depth: usize) -> Rect {
        let n = self.screen.node(c);
        let (ax, ay) = n.anchor.axes();
        let size = self.outer(c, item, room.size, depth + 1);
        let place = |a: AxisAnchor, start: f32, len: f32, size: f32, off: i16| match a {
            AxisAnchor::Stretch => {
                let o = abs(off as f32);
                (start + o, (len - 2.0 * o).max(0.0))
            }
            AxisAnchor::At(f) => (start + f * (len - size) + off as f32, size),
        };
        let (x, w) = place(ax, room.pos.x, room.size.x, size.x, n.offset.0);
        let (y, h) = place(ay, room.pos.y, room.size.y, size.y, n.offset.1);
        Rect::new(x, y, w, h)
    }

    #[allow(clippy::too_many_arguments)]
    fn stack(
        &mut self,
        i: usize,
        item: u16,
        slot: u16,
        content: Rect,
        on: Screen,
        depth: usize,
        clip: Option<Rect>,
        dir: Dir,
        gap: i16,
        align: Align,
        justify: Justify,
    ) -> f32 {
        let gap = gap as f32;
        let (mut fixed, mut fills, mut k) = (0.0f32, 0u32, 0u32);
        for c in self.kids(i, item) {
            if axis_size(&self.screen.node(c), dir) == Size::Fill {
                fills += 1;
            } else {
                fixed += axis(self.outer(c, item, content.size, depth + 1), dir).0;
            }
            k += 1;
        }
        let (main_len, cross_len) = axis(content.size, dir);
        let (main_pos, cross_pos) = axis(content.pos, dir);
        let gaps = if k > 1 { gap * (k - 1) as f32 } else { 0.0 };
        let free = main_len - fixed - gaps;
        let fill_each = if fills > 0 {
            free.max(0.0) / fills as f32
        } else {
            0.0
        };
        let (mut pos, between) = if fills > 0 || free <= 0.0 {
            (main_pos, gap)
        } else {
            match justify {
                Justify::Start => (main_pos, gap),
                Justify::Center => (main_pos + free * 0.5, gap),
                Justify::End => (main_pos + free, gap),
                Justify::SpaceBetween if k > 1 => (main_pos, gap + free / (k - 1) as f32),
                Justify::SpaceBetween => (main_pos, gap),
            }
        };
        let start = pos;
        let screen = self.screen;
        for c in screen.children(i) {
            let cn = self.screen.node(c);
            if !self.visible(&cn, item) {
                continue;
            }
            let s = self.outer(c, item, content.size, depth + 1);
            let (mut m, mut x) = axis(s, dir);
            if axis_size(&cn, dir) == Size::Fill {
                m = fill_each;
            }
            let stretch = align == Align::Stretch || cross_size(&cn, dir) == Size::Fill;
            if stretch {
                x = cross_len;
            }
            let xp = match align {
                _ if stretch => cross_pos,
                Align::Start | Align::Stretch => cross_pos,
                Align::Center => cross_pos + (cross_len - x) * 0.5,
                Align::End => cross_pos + cross_len - x,
            };
            let off = Vec2::new(cn.offset.0 as f32, cn.offset.1 as f32);
            let r = match dir {
                Dir::Horizontal => Rect::new(pos + off.x, xp + off.y, m, x),
                Dir::Vertical => Rect::new(xp + off.x, pos + off.y, x, m),
            };
            let c_on = self.screen_of(&cn, on);
            self.place(c, item, slot, r, c_on, depth + 1, clip);
            pos += m + between;
        }
        // What the children took along the stack.
        (pos - start - if k > 0 { between } else { 0.0 }).max(0.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn grid(
        &mut self,
        i: usize,
        item: u16,
        slot: u16,
        content: Rect,
        on: Screen,
        depth: usize,
        clip: Option<Rect>,
        cols: u8,
        gap: (i16, i16),
    ) {
        let cols = cols.clamp(1, 32) as usize;
        let (gx, gy) = (gap.0 as f32, gap.1 as f32);
        let cell_w = ((content.size.x - gx * (cols - 1) as f32) / cols as f32).max(0.0);
        let cell = Vec2::new(cell_w, content.size.y);
        let mut row = [0u16; 32];
        let mut y = content.pos.y;
        let screen = self.screen;
        let end = screen.node(i).end as usize;
        let mut next = i + 1;
        loop {
            let mut k = 0;
            while k < cols && next < end {
                let c = next;
                next = (screen.node(c).end as usize).max(c + 1);
                if self.visible(&screen.node(c), item) {
                    row[k] = c as u16;
                    k += 1;
                }
            }
            if k == 0 {
                break;
            }
            let row_h = row[..k]
                .iter()
                .map(|&c| self.outer(c as usize, item, cell, depth + 1).y)
                .fold(0.0f32, f32::max);
            for (col, &c) in row[..k].iter().enumerate() {
                let c = c as usize;
                let cn = screen.node(c);
                let mut s = self.outer(c, item, cell, depth + 1);
                if cn.size.0 == Size::Fill {
                    s.x = cell_w;
                }
                if cn.size.1 == Size::Fill {
                    s.y = row_h;
                }
                let fx = match cn.anchor.axes().0 {
                    AxisAnchor::At(f) => f,
                    AxisAnchor::Stretch => {
                        s.x = cell_w;
                        0.0
                    }
                };
                let x0 = content.pos.x + col as f32 * (cell_w + gx);
                let r = Rect::new(
                    x0 + fx * (cell_w - s.x) + cn.offset.0 as f32,
                    y + cn.offset.1 as f32,
                    s.x,
                    s.y,
                );
                let c_on = self.screen_of(&cn, on);
                self.place(c, item, slot, r, c_on, depth + 1, clip);
            }
            y += row_h + gy;
        }
    }

    /// The scroll entry of `node`, made on first use.
    fn scroll_slot(&mut self, node: u16, vertical: bool) -> Option<usize> {
        if let Some(k) = self.ui.scroll.iter().position(|s| s.node == node) {
            self.ui.scroll[k].vertical = vertical;
            return Some(k);
        }
        let k = self.ui.scroll.iter().position(|s| s.node == NONE)?;
        debug_assert!(k < MAX_SCROLLS);
        self.ui.scroll[k] = ScrollBox {
            node,
            offset: 0.0,
            max: 0.0,
            vertical,
        };
        Some(k)
    }

    /// Overflow flags of a text-bearing node in `content`.
    fn text_fit(&self, n: &UiNode<'_>, item: u16, content: Rect) -> u8 {
        let room = match n.kind {
            NodeKind::Toggle => content.size.x - self.line_height(n) - INNER_GAP,
            NodeKind::Slider { .. } => content.size.x - TRACK_MIN - INNER_GAP,
            _ => content.size.x,
        };
        let need = self.label_size(n, item, room.max(0.0));
        let wide = need.x > room + 0.5;
        let tall = need.y > content.size.y + 0.5;
        match n.overflow {
            Overflow::Ellipsis if wide => flags::TRUNCATED,
            Overflow::Ellipsis => 0,
            Overflow::Wrap if tall || wide => flags::OVERFLOW,
            Overflow::Visible | Overflow::Clip if wide || tall => flags::OVERFLOW,
            _ => 0,
        }
    }
}

fn axis_size(n: &UiNode<'_>, dir: Dir) -> Size {
    match dir {
        Dir::Horizontal => n.size.0,
        Dir::Vertical => n.size.1,
    }
}

fn cross_size(n: &UiNode<'_>, dir: Dir) -> Size {
    match dir {
        Dir::Horizontal => n.size.1,
        Dir::Vertical => n.size.0,
    }
}

fn intersect(a: &Rect, b: &Rect) -> Rect {
    let x0 = a.pos.x.max(b.pos.x);
    let y0 = a.pos.y.max(b.pos.y);
    let x1 = (a.pos.x + a.size.x).min(b.pos.x + b.size.x);
    let y1 = (a.pos.y + a.size.y).min(b.pos.y + b.size.y);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

pub(super) fn abs(v: f32) -> f32 {
    if v < 0.0 { -v } else { v }
}

/// An image's drawn rect inside `r` (pixels) at pixel scale `scale`.
pub(super) fn image_rect(r: Rect, src: (u16, u16), fit: ImageFit, scale: f32) -> Rect {
    let (sw, sh) = (src.0.max(1) as f32, src.1.max(1) as f32);
    let k = match fit {
        ImageFit::Stretch => return r,
        ImageFit::Contain => (r.size.x / sw).min(r.size.y / sh),
        ImageFit::Integer => {
            let k = floor((r.size.x / sw).min(r.size.y / sh));
            if k >= 1.0 { k } else { scale.min(1.0) }
        }
    };
    let (w, h) = (sw * k, sh * k);
    Rect::new(
        floor(r.pos.x + (r.size.x - w) * 0.5),
        floor(r.pos.y + (r.size.y - h) * 0.5),
        w,
        h,
    )
}
