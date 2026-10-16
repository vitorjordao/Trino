//! Menu navigation: a selection with key repeat over the stick, the d-pad
//! and the PC's arrows, confirm/back/extra buttons, and pointer taps on
//! registered hit boxes (the PC's mouse, the 3DS's touch screen).

use trino_core::{Button, InputState, PointerButton, Rect, Screen, Vec2};

/// One frame's menu input after repeat handling.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub confirm: bool,
    pub back: bool,
    pub x: bool,
    pub y: bool,
    pub l: bool,
    pub r: bool,
    pub start: bool,
}

/// Seconds before a held direction repeats, and between repeats.
const REPEAT_DELAY: f32 = 0.34;
const REPEAT_EVERY: f32 = 0.085;

/// Held-direction timers.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Repeat {
    held: [f32; 4],
}

impl Repeat {
    /// Edges and repeats of the four directions, plus button presses.
    pub fn nav(&mut self, now: &InputState, prev: &InputState, dt: f32) -> Nav {
        let dir = |s: &InputState| -> [bool; 4] {
            [
                s.is_down(Button::DpadUp) || s.stick.y > 0.55,
                s.is_down(Button::DpadDown) || s.stick.y < -0.55,
                s.is_down(Button::DpadLeft) || s.stick.x < -0.55,
                s.is_down(Button::DpadRight) || s.stick.x > 0.55,
            ]
        };
        let (d_now, d_prev) = (dir(now), dir(prev));
        let mut fire = [false; 4];
        for k in 0..4 {
            if d_now[k] && !d_prev[k] {
                fire[k] = true;
                self.held[k] = 0.0;
            } else if d_now[k] {
                let before = self.held[k];
                self.held[k] += dt;
                if self.held[k] >= REPEAT_DELAY {
                    let n0 = ((before - REPEAT_DELAY).max(-REPEAT_EVERY) / REPEAT_EVERY) as i32;
                    let n1 = ((self.held[k] - REPEAT_DELAY) / REPEAT_EVERY) as i32;
                    fire[k] = n1 > n0 || before < REPEAT_DELAY;
                }
            } else {
                self.held[k] = 0.0;
            }
        }
        let press = |b: Button| now.just_pressed(prev, b);
        Nav {
            up: fire[0],
            down: fire[1],
            left: fire[2],
            right: fire[3],
            confirm: press(Button::A),
            back: press(Button::B),
            x: press(Button::X),
            y: press(Button::Y),
            l: press(Button::L),
            r: press(Button::R),
            start: press(Button::Start),
        }
    }
}

/// Move a selection through `n` items (wrapping).
pub fn step(sel: &mut usize, n: usize, nav: &Nav) {
    if n == 0 {
        *sel = 0;
        return;
    }
    if nav.up {
        *sel = (*sel + n - 1) % n;
    }
    if nav.down {
        *sel = (*sel + 1) % n;
    }
}

/// -1, 0 or +1 from left/right.
pub fn lr(nav: &Nav) -> i32 {
    nav.right as i32 - nav.left as i32
}

/// A rectangle (screen pixels) the pointer can tap on `screen`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub screen: Screen,
    pub id: u8,
}

/// Up to 24 hit boxes registered by the last frame's drawing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hits {
    boxes: [Hit; 24],
    n: usize,
}

impl Default for Hits {
    fn default() -> Hits {
        Hits {
            boxes: [Hit {
                rect: Rect::new(0.0, 0.0, 0.0, 0.0),
                screen: Screen::Top,
                id: 0,
            }; 24],
            n: 0,
        }
    }
}

impl Hits {
    pub fn clear(&mut self) {
        self.n = 0;
    }

    pub fn add(&mut self, rect: Rect, screen: Screen, id: u8) {
        if self.n < self.boxes.len() {
            self.boxes[self.n] = Hit { rect, screen, id };
            self.n += 1;
        }
    }

    /// The box tapped this frame (a fresh primary press over it).
    pub fn tapped(&self, now: &InputState, prev: &InputState) -> Option<u8> {
        if !now.pointer_pressed(prev, PointerButton::Primary) {
            return None;
        }
        for h in &self.boxes[..self.n] {
            if let Some(p) = now.pointer_on(h.screen)
                && h.rect.contains(p)
            {
                return Some(h.id);
            }
        }
        None
    }

    /// The box under the pointer (hover highlights).
    pub fn hover(&self, now: &InputState) -> Option<u8> {
        for h in &self.boxes[..self.n] {
            if let Some(p) = now.pointer_on(h.screen)
                && h.rect.contains(p)
            {
                return Some(h.id);
            }
        }
        None
    }
}

/// Scroll a list window of `rows` so `sel` stays visible.
pub fn scroll(top: &mut usize, sel: usize, rows: usize) {
    if sel < *top {
        *top = sel;
    } else if sel >= *top + rows {
        *top = sel + 1 - rows;
    }
}

/// Screen point of the design canvas (for hit boxes).
pub fn canvas_rect(origin: Vec2, scale: f32, r: Rect) -> Rect {
    Rect::new(
        origin.x + r.pos.x * scale,
        origin.y + r.pos.y * scale,
        r.size.x * scale,
        r.size.y * scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(b: &[Button]) -> InputState {
        let mut s = InputState::default();
        for x in b {
            s.set(*x, true);
        }
        s
    }

    #[test]
    fn a_held_direction_repeats_after_a_delay() {
        let mut r = Repeat::default();
        let down = st(&[Button::DpadDown]);
        let none = InputState::default();
        let mut fires = 0;
        let mut prev = none;
        for _ in 0..60 {
            // One second at 60 fps.
            if r.nav(&down, &prev, 1.0 / 60.0).down {
                fires += 1;
            }
            prev = down;
        }
        // The press, then ~(1 - 0.34) / 0.085 ≈ 7 repeats.
        assert!((6..=10).contains(&fires), "{fires}");
        let mut sel = 0;
        step(
            &mut sel,
            3,
            &Nav {
                up: true,
                ..Nav::default()
            },
        );
        assert_eq!(sel, 2, "wraps");
    }

    #[test]
    fn taps_hit_registered_boxes_on_their_screen() {
        let mut h = Hits::default();
        h.add(Rect::new(10.0, 10.0, 20.0, 10.0), Screen::Bottom, 7);
        let mut tap = InputState::default();
        tap.pointer = Some(Vec2::new(15.0, 12.0));
        tap.pointer_screen = Screen::Bottom.index();
        tap.set_pointer_button(PointerButton::Primary, true);
        assert_eq!(h.tapped(&tap, &InputState::default()), Some(7));
        assert_eq!(h.tapped(&tap, &tap), None, "a hold is not a tap");
        tap.pointer_screen = Screen::Top.index();
        assert_eq!(h.tapped(&tap, &InputState::default()), None);
    }

    #[test]
    fn lists_scroll_to_the_selection() {
        let mut top = 0;
        scroll(&mut top, 9, 6);
        assert_eq!(top, 4);
        scroll(&mut top, 2, 6);
        assert_eq!(top, 2);
    }
}
