//! Input: the pad moves the focus by geometry, the pointer focuses and
//! clicks, touch taps. One code path for every target: an N64 has only the
//! pad, a 3DS adds the bottom screen's touch, a PC the mouse and keys.

use super::layout::{abs, floor};
use super::node::NodeKind;
use super::values::UiValue;
use super::{NONE, Slot, Ui, UiEvent, UiEvents, UiScreen, flags};
use crate::input::{Button, InputState, Key, PointerButton};
use crate::math::{Rect, Vec2};
use crate::render::Screen;

/// Seconds a held direction waits before repeating, then between repeats.
pub const REPEAT_DELAY: f32 = 0.4;
pub const REPEAT_EVERY: f32 = 0.1;
/// Stick deflection that counts as a direction.
const STICK: f32 = 0.5;
/// Pixels a wheel notch scrolls, in virtual units.
const WHEEL_STEP: f32 = 16.0;

const UP: u8 = 0;
const DOWN: u8 = 1;
const LEFT: u8 = 2;
const RIGHT: u8 = 3;

/// The direction held this frame: the D-pad first, then either stick.
fn held_dir(input: &InputState) -> Option<u8> {
    let pad = [
        (Button::DpadUp, UP),
        (Button::DpadDown, DOWN),
        (Button::DpadLeft, LEFT),
        (Button::DpadRight, RIGHT),
    ];
    if let Some((_, d)) = pad.iter().find(|(b, _)| input.is_down(*b)) {
        return Some(*d);
    }
    for s in [input.stick, input.right_stick] {
        let (ax, ay) = (abs(s.x), abs(s.y));
        if ax.max(ay) < STICK {
            continue;
        }
        // Stick y is up-positive.
        return Some(if ax > ay {
            if s.x > 0.0 { RIGHT } else { LEFT }
        } else if s.y > 0.0 {
            UP
        } else {
            DOWN
        });
    }
    None
}

impl<const N: usize> Ui<N> {
    /// One frame of input against the last layout: focus moves, presses,
    /// value changes. `prev` is the previous frame's input (edges), `dt`
    /// the frame time (held directions repeat). Returns what happened.
    ///
    /// Before the first [`draw`](Self::draw) (or after a [`reset`](Self::reset))
    /// it lays the screen out itself, for the last target drawn to (its
    /// reference size at first) with text measured as [`CellMeasure`]:
    /// menus work headless, and the next draw lays out for real.
    pub fn update<'a>(
        &mut self,
        screen: &UiScreen<'a>,
        input: &InputState,
        prev: &InputState,
        dt: f32,
    ) -> UiEvents<'a> {
        let mut events = UiEvents::new();
        self.time += dt;
        if !self.laid_out {
            // Not drawn yet (or reset): lay out for the last target with
            // the engine font's cell, so the menu works before its draw.
            let env = self
                .last_env
                .unwrap_or_else(|| super::EnvHint::reference(screen))
                .env();
            self.layout(screen, &env, &super::CellMeasure);
        }
        if self.count == 0 {
            return events;
        }
        if self
            .focus_slot()
            .is_none_or(|i| !self.slots[i].has(flags::FOCUSABLE))
        {
            self.focus = self
                .initial_focus(screen)
                .map(|i| (self.slots[i].node, self.slots[i].item));
        }
        let before = self.focus;

        // Pointer: hover focuses (when it moves), press + release clicks.
        for on in [Screen::Top, Screen::Bottom] {
            self.pointer(screen, input, prev, on, &mut events);
        }

        // Pad and keys.
        let dir = held_dir(input);
        let fire = if dir != self.hold_dir {
            self.hold_dir = dir;
            self.hold_t = 0.0;
            dir.is_some()
        } else if dir.is_some() {
            self.hold_t += dt;
            if self.hold_t >= REPEAT_DELAY {
                self.hold_t -= REPEAT_EVERY;
                true
            } else {
                false
            }
        } else {
            false
        };
        if let (true, Some(d)) = (fire, dir)
            && let Some(at) = self.focus_slot()
        {
            let node = screen.node(self.slots[at].node as usize);
            let adjusts = matches!(node.kind, NodeKind::Slider { .. } | NodeKind::Toggle);
            if adjusts && (d == LEFT || d == RIGHT) {
                self.adjust(screen, at, if d == LEFT { -1 } else { 1 }, &mut events);
            } else if let Some(next) = self.nav_target(screen, at, d, screen.wrap) {
                self.focus = Some((self.slots[next].node, self.slots[next].item));
            }
        }

        let accept = input.just_pressed(prev, Button::A)
            || input.key_pressed(prev, Key::Enter)
            || input.key_pressed(prev, Key::Space);
        self.accept_down = input.is_down(Button::A);
        if accept && let Some(at) = self.focus_slot() {
            self.activate(screen, at, &mut events);
        }
        if input.just_pressed(prev, Button::B)
            || input.key_pressed(prev, Key::Escape)
            || input.key_pressed(prev, Key::Backspace)
        {
            events.push(UiEvent::Back {
                action: screen.on_back,
            });
        }

        if self.focus != before
            && let Some(at) = self.focus_slot()
        {
            let s = self.slots[at];
            events.push(UiEvent::Moved {
                id: screen.node(s.node as usize).id,
                item: s.item,
            });
            self.scroll_into_view(at);
        }
        events
    }

    fn pointer<'a>(
        &mut self,
        screen: &UiScreen<'a>,
        input: &InputState,
        prev: &InputState,
        on: Screen,
        events: &mut UiEvents<'a>,
    ) {
        let k = on.index() as usize;
        let p = input.pointer_on(on);
        let last = self.pointer_last[k];
        self.pointer_last[k] = p;
        let at = p.or(last);
        let Some(at) = at else {
            return;
        };
        let hit = self.hit(on, at);
        if let (Some(p), Some(h)) = (p, hit)
            && last != Some(p)
        {
            self.focus = Some((self.slots[h].node, self.slots[h].item));
        }
        if input.pointer_pressed(prev, PointerButton::Primary)
            && let Some(h) = hit
        {
            self.pressed = Some((self.slots[h].node, self.slots[h].item));
            self.focus = self.pressed;
        }
        if let Some(pressed) = self.pressed
            && let Some(i) = self.slot_of(pressed)
            && self.slots[i].on == on
        {
            let node = screen.node(pressed.0 as usize);
            if let NodeKind::Slider { .. } = node.kind
                && input.pointer_down(PointerButton::Primary)
            {
                self.drag(screen, i, at.x, events);
            }
            if input.pointer_released(prev, PointerButton::Primary) {
                self.pressed = None;
                if hit == Some(i) && !matches!(node.kind, NodeKind::Slider { .. }) {
                    self.activate(screen, i, events);
                }
            }
        }
        if input.wheel != 0.0
            && let Some(s) = self.scroll_at(on, at)
        {
            let node = self.slots[s].node;
            if let Some(e) = self.scroll.iter_mut().find(|e| e.node == node) {
                e.offset = (e.offset - input.wheel * WHEEL_STEP).clamp(0.0, e.max);
            }
        }
    }

    /// The topmost enabled control under `p` on screen `on`.
    fn hit(&self, on: Screen, p: Vec2) -> Option<usize> {
        (0..self.count).rev().find(|&i| {
            let s = &self.slots[i];
            s.on == on && s.has(flags::FOCUSABLE) && !self.hidden(i) && self.rects[i].contains(p)
        })
    }

    /// The innermost scroll box under `p`.
    fn scroll_at(&self, on: Screen, p: Vec2) -> Option<usize> {
        (0..self.count).rev().find(|&i| {
            let s = &self.slots[i];
            s.on == on
                && self
                    .scroll
                    .iter()
                    .any(|e| e.node == s.node && e.node != NONE)
                && self.rects[i].contains(p)
        })
    }

    fn slot_of(&self, (node, item): (u16, u16)) -> Option<usize> {
        self.slots()
            .iter()
            .position(|s| s.node == node && s.item == item)
    }

    /// A, a click or a tap on slot `at`.
    fn activate<'a>(&mut self, screen: &UiScreen<'a>, at: usize, events: &mut UiEvents<'a>) {
        let s = self.slots[at];
        if !s.has(flags::FOCUSABLE) {
            return;
        }
        let node = screen.node(s.node as usize);
        if let NodeKind::Toggle = node.kind {
            self.adjust(screen, at, 0, events);
        }
        if !node.action.is_empty() || matches!(node.kind, NodeKind::Button) {
            events.push(UiEvent::Click {
                action: node.action,
                id: node.id,
                item: s.item,
            });
        }
    }

    /// Move a slider by `step` steps, or flip a toggle (`step` 0: flip; ±1:
    /// set off/on).
    fn adjust<'a>(
        &mut self,
        screen: &UiScreen<'a>,
        at: usize,
        step: i32,
        events: &mut UiEvents<'a>,
    ) {
        let s = self.slots[at];
        let node = screen.node(s.node as usize);
        if node.bind.is_empty() {
            return;
        }
        let now = self.values.get(node.bind, s.item);
        let (value, new) = match node.kind {
            NodeKind::Toggle => {
                let on = now.is_some_and(|v| v.truthy());
                let next = match step {
                    0 => !on,
                    s => s > 0,
                };
                (i32::from(next), UiValue::Bool(next))
            }
            NodeKind::Slider { min, max, step: by } => {
                let v = now.and_then(|v| v.int()).unwrap_or(min);
                let next = (v + step * by.max(1)).clamp(min.min(max), max.max(min));
                (next, UiValue::Int(next))
            }
            _ => return,
        };
        if now == Some(new) {
            return;
        }
        self.values.put(node.bind, s.item, new);
        events.push(UiEvent::Changed {
            bind: node.bind,
            value,
            item: s.item,
        });
    }

    /// A slider dragged to pixel `x`.
    fn drag<'a>(&mut self, screen: &UiScreen<'a>, at: usize, x: f32, events: &mut UiEvents<'a>) {
        let s = self.slots[at];
        let node = screen.node(s.node as usize);
        let NodeKind::Slider { min, max, step } = node.kind else {
            return;
        };
        let Some(track) = self.track(s.node, s.item) else {
            return;
        };
        if track.size.x <= 0.0 || max <= min {
            return;
        }
        let t = ((x - track.pos.x) / track.size.x).clamp(0.0, 1.0);
        let step = step.max(1);
        let raw = min as f32 + t * (max - min) as f32;
        let snapped = min + (floor((raw - min as f32) / step as f32 + 0.5) as i32) * step;
        let next = snapped.clamp(min, max);
        let now = self.values.get(node.bind, s.item).and_then(|v| v.int());
        if now == Some(next) {
            return;
        }
        self.values.put(node.bind, s.item, UiValue::Int(next));
        events.push(UiEvent::Changed {
            bind: node.bind,
            value: next,
            item: s.item,
        });
    }

    /// The slot the focus starts on: the screen's `focus` node, else the
    /// first enabled control.
    pub(super) fn initial_focus(&self, screen: &UiScreen<'_>) -> Option<usize> {
        let wanted = (screen.focus != NONE)
            .then(|| {
                self.slots()
                    .iter()
                    .position(|s| s.node == screen.focus && s.has(flags::FOCUSABLE))
            })
            .flatten();
        wanted.or_else(|| self.slots().iter().position(|s| s.has(flags::FOCUSABLE)))
    }

    /// Slot `i`'s rect in one plane for both screens: the bottom screen
    /// sits under the top one, centered.
    fn plane(&self, i: usize) -> Rect {
        let r = self.rects[i];
        if self.slots[i].on == Screen::Top {
            return r;
        }
        let top = self.frames[0].size * self.frames[0].scale;
        let bottom = self.frames[1].size * self.frames[1].scale;
        Rect::new(
            r.pos.x + (top.x - bottom.x) * 0.5,
            r.pos.y + top.y,
            r.size.x,
            r.size.y,
        )
    }

    /// Where the focus goes from slot `from` in direction `dir` (0 up,
    /// 1 down, 2 left, 3 right): the node's override, else the nearest
    /// control that way (ranges that overlap across the move first), else
    /// — with `wrap` — the farthest one the other way.
    pub(super) fn nav_target(
        &self,
        screen: &UiScreen<'_>,
        from: usize,
        dir: u8,
        wrap: bool,
    ) -> Option<usize> {
        let node = screen.node(self.slots[from].node as usize);
        let explicit = node.nav[dir as usize];
        if explicit != NONE {
            return self
                .slots()
                .iter()
                .position(|s| s.node == explicit && s.has(flags::FOCUSABLE));
        }
        let a = self.plane(from);
        let (ac, vertical) = (center(&a), dir == UP || dir == DOWN);
        let sign = if dir == DOWN || dir == RIGHT {
            1.0
        } else {
            -1.0
        };
        let mut best: Option<(f32, usize)> = None;
        let mut far: Option<(f32, f32, usize)> = None;
        for i in 0..self.count {
            let s: &Slot = &self.slots[i];
            if i == from || !s.has(flags::FOCUSABLE) {
                continue;
            }
            let b = self.plane(i);
            let bc = center(&b);
            let (d_main, gap) = if vertical {
                (bc.y - ac.y, range_gap(a.pos.x, a.size.x, b.pos.x, b.size.x))
            } else {
                (bc.x - ac.x, range_gap(a.pos.y, a.size.y, b.pos.y, b.size.y))
            };
            let along = d_main * sign;
            if along > 0.5 {
                let off = if vertical { bc.x - ac.x } else { bc.y - ac.y };
                let score = along + 2.0 * gap + 0.01 * abs(off);
                if best.is_none_or(|(s, _)| score < s) {
                    best = Some((score, i));
                }
            } else if wrap && along < -0.5 {
                // The farthest back, in the same row or column first.
                if far.is_none_or(|(g, d, _)| gap < g || (gap == g && -along > d)) {
                    far = Some((gap, -along, i));
                }
            }
        }
        best.map(|b| b.1).or(far.map(|f| f.2))
    }

    /// Scroll the boxes around slot `at` so it shows.
    fn scroll_into_view(&mut self, at: usize) {
        let r = self.rects[at];
        let mut p = self.slots[at].parent;
        while p != NONE {
            let ps = self.slots[p as usize];
            if let Some(k) = self
                .scroll
                .iter()
                .position(|e| e.node == ps.node && e.node != NONE)
            {
                let view = self.rects[p as usize];
                let scale = self.frames[ps.on.index() as usize].scale;
                let vertical = self.scroll[k].vertical;
                let (lo, len, vlo, vlen) = if vertical {
                    (r.pos.y, r.size.y, view.pos.y, view.size.y)
                } else {
                    (r.pos.x, r.size.x, view.pos.x, view.size.x)
                };
                let delta = if lo < vlo {
                    lo - vlo
                } else if lo + len > vlo + vlen {
                    lo + len - (vlo + vlen)
                } else {
                    0.0
                };
                if delta != 0.0 {
                    let e = &mut self.scroll[k];
                    // The layout key holds the offsets: the next draw lays
                    // out again.
                    e.offset = (e.offset + delta / scale).max(0.0);
                }
                return;
            }
            p = ps.parent;
        }
    }
}

fn center(r: &Rect) -> Vec2 {
    Vec2::new(r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5)
}

/// The gap between two ranges (0 when they overlap).
fn range_gap(a: f32, alen: f32, b: f32, blen: f32) -> f32 {
    if b > a + alen {
        b - (a + alen)
    } else if a > b + blen {
        a - (b + blen)
    } else {
        0.0
    }
}
