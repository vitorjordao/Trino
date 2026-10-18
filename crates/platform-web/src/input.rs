//! [`trino_core::Input`] from browser events: keyboard (`KeyboardEvent.code`,
//! the PC's bindings — [`crate::keys`]), mouse with Pointer Lock, touch,
//! and gamepads polled once a frame (the Gamepad API has no events for
//! buttons).
//!
//! Positions arrive in CSS pixels relative to the canvas; the runtime turns
//! them into the canvas's device pixels and the renderer into game pixels
//! (`PcRenderer::window_to_pointer`, the same letterboxed integer upscale
//! the PC window uses). Under Pointer Lock the cursor is hidden and the
//! mouse moves a virtual pointer by its deltas, clamped to the canvas.

use std::cell::RefCell;
use std::rc::Rc;

use trino_core::{GamepadStatus, Input, InputState, PointerButton, Vec2};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, KeyboardEvent, MouseEvent, TouchEvent, WheelEvent};

use crate::keys::{Keyboard, PadReading, is_game_key, merge_pad};

/// What the event handlers write and [`WebInput::poll`] reads.
#[derive(Default)]
pub(crate) struct Shared {
    keyboard: Keyboard,
    /// The pointer in the canvas's device pixels, `None` off the canvas.
    pub(crate) pointer_px: Option<(f64, f64)>,
    buttons: [bool; 3],
    wheel: f32,
    /// A gesture happened (the runtime resumes audio on it).
    pub(crate) gesture: bool,
    /// The canvas's device-pixel size (Pointer Lock's virtual cursor stays
    /// inside it) and its device-pixel ratio.
    pub(crate) canvas_px: (f64, f64),
    pub(crate) dpr: f64,
}

pub struct WebInput {
    pub(crate) shared: Rc<RefCell<Shared>>,
    /// The game's picture: the runtime maps `pointer_px` through the
    /// renderer before each poll.
    pub(crate) pointer: Option<(Vec2, u8)>,
    pad_status: GamepadStatus,
    pad_name: Option<String>,
    _listeners: Vec<Listener>,
}

/// An event listener kept alive for the page's life.
struct Listener {
    _closure: Box<dyn std::any::Any>,
}

/// Is the pointer locked to `canvas`?
pub(crate) fn locked_to(canvas: &HtmlCanvasElement) -> bool {
    let target: &JsValue = canvas.as_ref();
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.pointer_lock_element())
        .is_some_and(|el| AsRef::<JsValue>::as_ref(&el) == target)
}

fn listen<E: JsCast + wasm_bindgen::convert::FromWasmAbi + 'static>(
    target: &web_sys::EventTarget,
    event: &str,
    passive: bool,
    f: impl FnMut(E) + 'static,
) -> Listener {
    let closure = Closure::<dyn FnMut(E)>::new(f);
    let opts = web_sys::AddEventListenerOptions::new();
    opts.set_passive(passive);
    let _ = target.add_event_listener_with_callback_and_add_event_listener_options(
        event,
        closure.as_ref().unchecked_ref(),
        &opts,
    );
    Listener {
        _closure: Box::new(closure),
    }
}

impl WebInput {
    /// Listen on `canvas` (pointer, touch, wheel) and the window (keys, so
    /// they work without clicking the canvas first; focus loss).
    pub fn attach(canvas: &HtmlCanvasElement) -> Self {
        let shared = Rc::new(RefCell::new(Shared {
            dpr: 1.0,
            ..Shared::default()
        }));
        let window: web_sys::EventTarget = web_sys::window().expect("a window").into();
        let canvas_t: &web_sys::EventTarget = canvas.as_ref();
        let mut listeners = Vec::new();

        let s = shared.clone();
        listeners.push(listen(
            &window,
            "keydown",
            false,
            move |e: KeyboardEvent| {
                let code = e.code();
                // Ctrl/Cmd shortcuts (reload, devtools, copy) stay the browser's.
                if is_game_key(&code) && !e.ctrl_key() && !e.meta_key() {
                    e.prevent_default();
                }
                let mut s = s.borrow_mut();
                s.gesture = true;
                if !e.repeat() {
                    s.keyboard.key(&code, true);
                }
            },
        ));
        let s = shared.clone();
        listeners.push(listen(&window, "keyup", false, move |e: KeyboardEvent| {
            if is_game_key(&e.code()) {
                e.prevent_default();
            }
            s.borrow_mut().keyboard.key(&e.code(), false);
        }));
        // Any click on the page (the audio banner, the toolbar) is a
        // gesture that may unlock audio.
        let s = shared.clone();
        listeners.push(listen(
            &window,
            "pointerdown",
            true,
            move |_: web_sys::PointerEvent| {
                s.borrow_mut().gesture = true;
            },
        ));
        let s = shared.clone();
        listeners.push(listen(
            &window,
            "blur",
            true,
            move |_: web_sys::FocusEvent| {
                let mut s = s.borrow_mut();
                s.keyboard.release_all();
                s.buttons = [false; 3];
            },
        ));

        let s = shared.clone();
        let c = canvas.clone();
        listeners.push(listen(canvas_t, "mousemove", true, move |e: MouseEvent| {
            let mut s = s.borrow_mut();
            let locked = locked_to(&c);
            if locked {
                // A virtual cursor moved by the deltas, inside the canvas.
                let (x, y) = s
                    .pointer_px
                    .unwrap_or((s.canvas_px.0 / 2.0, s.canvas_px.1 / 2.0));
                let dpr = s.dpr;
                s.pointer_px = Some((
                    (x + e.movement_x() as f64 * dpr).clamp(0.0, s.canvas_px.0 - 1.0),
                    (y + e.movement_y() as f64 * dpr).clamp(0.0, s.canvas_px.1 - 1.0),
                ));
            } else {
                s.pointer_px = Some((e.offset_x() as f64 * s.dpr, e.offset_y() as f64 * s.dpr));
            }
        }));
        let s = shared.clone();
        let c = canvas.clone();
        listeners.push(listen(
            canvas_t,
            "mouseleave",
            true,
            move |_: MouseEvent| {
                let locked = locked_to(&c);
                if !locked {
                    s.borrow_mut().pointer_px = None;
                }
            },
        ));
        for (event, down) in [("mousedown", true), ("mouseup", false)] {
            let s = shared.clone();
            let c = canvas.clone();
            listeners.push(listen(canvas_t, event, false, move |e: MouseEvent| {
                if down {
                    // Keys go to the game after a click on it.
                    let _ = c.focus();
                }
                let mut s = s.borrow_mut();
                s.gesture = true;
                if let Some(slot) = match e.button() {
                    0 => Some(0),
                    2 => Some(1),
                    1 => Some(2),
                    _ => None,
                } {
                    s.buttons[slot] = down;
                }
            }));
        }
        listeners.push(listen(canvas_t, "contextmenu", false, |e: MouseEvent| {
            // The right button is a game button.
            e.prevent_default();
        }));
        let s = shared.clone();
        listeners.push(listen(canvas_t, "wheel", false, move |e: WheelEvent| {
            e.prevent_default();
            // Lines are notches; pixels (touchpads) ~40 a notch; pages 3.
            let notches = match e.delta_mode() {
                WheelEvent::DOM_DELTA_LINE => -e.delta_y(),
                WheelEvent::DOM_DELTA_PAGE => -e.delta_y() * 3.0,
                _ => -e.delta_y() / 40.0,
            };
            s.borrow_mut().wheel += notches as f32;
        }));

        // Touch: the first finger is the pointer and the primary button.
        // Not passive: preventDefault stops the page from scrolling or
        // zooming under the game.
        for (event, down) in [
            ("touchstart", Some(true)),
            ("touchmove", None),
            ("touchend", Some(false)),
            ("touchcancel", Some(false)),
        ] {
            let s = shared.clone();
            let c = canvas.clone();
            listeners.push(listen(canvas_t, event, false, move |e: TouchEvent| {
                e.prevent_default();
                let mut s = s.borrow_mut();
                s.gesture = true;
                let rect = c.get_bounding_client_rect();
                if let Some(t) = e.touches().get(0) {
                    s.pointer_px = Some((
                        (t.client_x() as f64 - rect.left()) * s.dpr,
                        (t.client_y() as f64 - rect.top()) * s.dpr,
                    ));
                }
                match down {
                    Some(true) => s.buttons[0] = true,
                    Some(false) if e.touches().length() == 0 => {
                        s.buttons[0] = false;
                        s.pointer_px = None;
                    }
                    _ => {}
                }
            }));
        }

        WebInput {
            shared,
            pointer: None,
            pad_status: GamepadStatus::Disconnected,
            pad_name: None,
            _listeners: listeners,
        }
    }

    /// Did a click, key or touch happen since the last call?
    pub fn take_gesture(&mut self) -> bool {
        std::mem::take(&mut self.shared.borrow_mut().gesture)
    }

    /// The gamepads: none seen yet, or connected.
    pub fn gamepad_status(&self) -> GamepadStatus {
        self.pad_status
    }

    pub fn gamepad_name(&self) -> Option<String> {
        self.pad_name.clone()
    }

    /// Every connected pad in the standard mapping.
    fn read_pads(&mut self) -> Vec<PadReading> {
        let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
            return Vec::new();
        };
        let Ok(list) = nav.get_gamepads() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        self.pad_name = None;
        for pad in list.iter() {
            let Ok(pad) = pad.dyn_into::<web_sys::Gamepad>() else {
                continue;
            };
            if !pad.connected() {
                continue;
            }
            if self.pad_name.is_none() {
                self.pad_name = Some(pad.id());
            }
            let buttons = pad
                .buttons()
                .iter()
                .map(|b| {
                    b.dyn_into::<web_sys::GamepadButton>()
                        .map_or((false, 0.0), |b| (b.pressed(), b.value() as f32))
                })
                .collect();
            let axes = pad
                .axes()
                .iter()
                .map(|a| a.as_f64().unwrap_or(0.0) as f32)
                .collect();
            out.push(PadReading { buttons, axes });
        }
        self.pad_status = if out.is_empty() {
            GamepadStatus::Disconnected
        } else {
            GamepadStatus::Connected
        };
        out
    }
}

impl Input for WebInput {
    fn poll(&mut self) -> InputState {
        let pads = self.read_pads();
        let mut s = self.shared.borrow_mut();
        let mut state = s.keyboard.state();
        for (i, b) in [
            PointerButton::Primary,
            PointerButton::Secondary,
            PointerButton::Middle,
        ]
        .into_iter()
        .enumerate()
        {
            state.set_pointer_button(b, s.buttons[i]);
        }
        state.wheel = std::mem::take(&mut s.wheel);
        if let Some((p, screen)) = self.pointer {
            state.pointer = Some(p);
            state.pointer_screen = screen;
        }
        // Keys bound to L2/R2 are fully pulled triggers (none by default).
        state.triggers = Vec2::new(
            state.is_down(trino_core::Button::L2) as u8 as f32,
            state.is_down(trino_core::Button::R2) as u8 as f32,
        );
        for pad in &pads {
            merge_pad(&mut state, pad);
        }
        state
    }
}
