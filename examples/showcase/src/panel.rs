//! The showcase's bottom-screen panel (3DS touch screen, and the PC's 3DS
//! profile): live renderer metrics and touch buttons that switch real
//! engine options — the camera, the light, the fog, the texture filter and
//! the animation clock. Every button also has a pad button, so the options
//! work on single-screen targets too (N64, PC native), where the panel
//! itself is never drawn (`Renderer::on_screen(Screen::Bottom)` says no).
//!
//! Layout and toggle logic are plain data and functions ([`Toggle::rect`],
//! [`hit`], [`Look::apply`]) so tests need no renderer; [`probe_panel`] and
//! [`check_panel`] read the drawn panel back on PC and in Azahar.

use crate::ShowcaseGame;
use core::fmt::Write as _;
use trino_core::{
    Audio, Button, Color, FrameStats, Game, InputState, Light, PointerButton, Rect, Renderer,
    Screen, TexFilter, TextAlign, TextBuf, TextParams, Vec2, Vec3,
};

/// The bottom screen's size (the 3DS touch screen).
pub const SIZE: Vec2 = Vec2::new(320.0, 240.0);
/// Title bar across the top.
pub const TITLE: Rect = Rect::new(0.0, 0.0, 320.0, 20.0);
/// Where the metric lines start, and their spacing (scale-2 text).
const METRICS_AT: Vec2 = Vec2::new(6.0, 23.0);
const LINE: f32 = 15.0;

/// Panel background, and the buttons' fills: at their default value, at
/// another value, and held. More than the probes' tolerance (24) apart in
/// some channel from each other and from the 3DS shim's bottom-screen
/// clear (16, 18, 28), so a probe can tell a drawn background from no draw.
pub const BG: Color = Color::rgb(30, 36, 64);
pub const BTN_DEFAULT: Color = Color::rgb(70, 74, 96);
pub const BTN_CHANGED: Color = Color::rgb(40, 110, 200);
pub const BTN_HELD: Color = Color::rgb(120, 170, 255);
const TITLE_BG: Color = Color::rgb(36, 44, 84);
const METRIC: Color = Color::rgb(200, 210, 230);

/// One option the panel switches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Toggle {
    /// Follow (the orbit), high (a steep orbit) or the scene's own camera.
    Camera,
    /// The scene's light, or a warm, dim, colored dusk light.
    Light,
    /// The scene's distance fog on or off.
    Fog,
    /// `Renderer::set_texture_filter`: the target default, point, bilinear.
    Filter,
    /// The animation clock (coins spinning, animated clips, sparks).
    Anim,
}

/// Every toggle, in layout order.
pub const TOGGLES: [Toggle; 5] = [
    Toggle::Camera,
    Toggle::Light,
    Toggle::Fog,
    Toggle::Filter,
    Toggle::Anim,
];

impl Toggle {
    /// The touch button (bottom-screen pixels): three on the first row,
    /// two wider ones under them, 5–6 px apart.
    pub const fn rect(self) -> Rect {
        match self {
            Toggle::Camera => Rect::new(5.0, 108.0, 100.0, 60.0),
            Toggle::Light => Rect::new(110.0, 108.0, 100.0, 60.0),
            Toggle::Fog => Rect::new(215.0, 108.0, 100.0, 60.0),
            Toggle::Filter => Rect::new(5.0, 174.0, 152.0, 60.0),
            Toggle::Anim => Rect::new(163.0, 174.0, 152.0, 60.0),
        }
    }

    /// The pad button that does the same (PC keys: X = C, Y = V, B = X,
    /// Select = Right Shift, Start = Enter).
    pub const fn button(self) -> Button {
        match self {
            Toggle::Camera => Button::X,
            Toggle::Light => Button::Y,
            Toggle::Fog => Button::B,
            Toggle::Filter => Button::Select,
            Toggle::Anim => Button::Start,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Toggle::Camera => "CAMERA",
            Toggle::Light => "LIGHT",
            Toggle::Fog => "FOG",
            Toggle::Filter => "FILTER",
            Toggle::Anim => "ANIM",
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Toggle::Camera => "X",
            Toggle::Light => "Y",
            Toggle::Fog => "B",
            Toggle::Filter => "SELECT",
            Toggle::Anim => "START",
        }
    }
}

/// The button under a bottom-screen point, if any.
pub fn hit(p: Vec2) -> Option<Toggle> {
    TOGGLES.into_iter().find(|t| t.rect().contains(p))
}

/// Camera presets ([`Look::camera`]).
pub const CAMERA_FOLLOW: u8 = 0;
pub const CAMERA_HIGH: u8 = 1;
pub const CAMERA_SCENE: u8 = 2;

/// What the panel switched: a few bytes in the game (console stacks are
/// small).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Look {
    /// [`CAMERA_FOLLOW`], [`CAMERA_HIGH`] or [`CAMERA_SCENE`].
    pub camera: u8,
    pub dusk: bool,
    pub fog: bool,
    pub filter: TexFilter,
    pub paused: bool,
}

impl Look {
    /// The scene as authored: what every target draws until a toggle.
    pub const DEFAULT: Look = Look {
        camera: CAMERA_FOLLOW,
        dusk: false,
        fog: true,
        filter: TexFilter::Default,
        paused: false,
    };

    /// Switch `t` to its next value.
    pub fn apply(&mut self, t: Toggle) {
        match t {
            Toggle::Camera => self.camera = (self.camera + 1) % 3,
            Toggle::Light => self.dusk = !self.dusk,
            Toggle::Fog => self.fog = !self.fog,
            Toggle::Filter => {
                self.filter = match self.filter {
                    TexFilter::Default => TexFilter::Point,
                    TexFilter::Point => TexFilter::Bilinear,
                    TexFilter::Bilinear => TexFilter::Default,
                }
            }
            Toggle::Anim => self.paused = !self.paused,
        }
    }

    /// `t` is away from its default value (the button shows it).
    pub fn changed(&self, t: Toggle) -> bool {
        let d = Look::DEFAULT;
        match t {
            Toggle::Camera => self.camera != d.camera,
            Toggle::Light => self.dusk != d.dusk,
            Toggle::Fog => self.fog != d.fog,
            Toggle::Filter => self.filter != d.filter,
            Toggle::Anim => self.paused != d.paused,
        }
    }

    fn value(&self, t: Toggle) -> &'static str {
        match t {
            Toggle::Camera => match self.camera {
                CAMERA_FOLLOW => "FOLLOW",
                CAMERA_HIGH => "HIGH",
                _ => "SCENE",
            },
            Toggle::Light if self.dusk => "DUSK",
            Toggle::Light => "SCENE",
            Toggle::Fog if self.fog => "ON",
            Toggle::Fog => "OFF",
            Toggle::Filter => match self.filter {
                TexFilter::Default => "DEFAULT",
                TexFilter::Point => "POINT",
                TexFilter::Bilinear => "BILINEAR",
            },
            Toggle::Anim if self.paused => "PAUSED",
            Toggle::Anim => "RUN",
        }
    }

    /// The light to draw with: the scene's own (exactly, at the default),
    /// or the dusk light, with or without the scene's fog.
    pub fn light(&self, scene: &Light) -> Light {
        let mut light = if self.dusk {
            Light {
                // Low sun from the side, warm; a cool, dim ambient floor.
                dir: Vec3::new(0.85, -0.3, 0.45),
                ambient: 0.25,
                color: Color::rgb(255, 170, 110),
                ambient_color: Color::rgb(110, 120, 190),
                fog: scene.fog,
            }
        } else {
            *scene
        };
        if !self.fog {
            light.fog = trino_core::Fog::NONE;
        }
        light
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The toggle tapped this frame: a fresh press of the pointer on the
/// bottom screen, over a button. Top-screen clicks never count.
pub fn tapped(input: &InputState, prev: &InputState) -> Option<Toggle> {
    if !input.pointer_pressed(prev, PointerButton::Primary) {
        return None;
    }
    input.pointer_on(Screen::Bottom).and_then(hit)
}

/// The pointer holds `t`'s button on the bottom screen.
fn held(input: &InputState, t: Toggle) -> bool {
    input.pointer_down(PointerButton::Primary)
        && input
            .pointer_on(Screen::Bottom)
            .is_some_and(|p| t.rect().contains(p))
}

/// The fill of `t`'s button for this state and pointer.
pub fn button_color(look: &Look, prev: &InputState, t: Toggle) -> Color {
    if held(prev, t) {
        BTN_HELD
    } else if look.changed(t) {
        BTN_CHANGED
    } else {
        BTN_DEFAULT
    }
}

/// Draw the panel. Called after `on_screen(Screen::Bottom)` succeeded:
/// 2D only, so no bottom-screen camera is needed. Out of line: its text
/// buffers stay off the frame of `render`, which single-screen targets
/// (the N64's 64 KB stack) run without ever calling it.
#[inline(never)]
pub fn draw(game: &ShowcaseGame, r: &mut dyn Renderer) {
    // Counters of the last finished frame (this panel's glyphs included):
    // every number has a fixed width, so the panel's own sprite count is
    // the same every frame and the readout settles after one frame.
    let s = r.stats();
    r.fill_rect(Rect::new(0.0, 0.0, SIZE.x, SIZE.y), BG);
    r.fill_rect(TITLE, TITLE_BG);
    let big = TextParams {
        scale: 2.0,
        ..TextParams::DEFAULT
    };
    r.draw_text(crate::FONT, Vec2::new(6.0, 1.0), "SHOWCASE", &big);
    let mut buf = TextBuf::<28>::new();
    let _ = write!(buf, "COINS {}/{}", game.coins, game.total_coins);
    r.draw_text(
        crate::FONT,
        Vec2::new(314.0, 1.0),
        buf.as_str(),
        &TextParams {
            align: TextAlign::Right,
            ..big
        },
    );

    let metric = TextParams {
        tint: METRIC,
        ..big
    };
    for (i, line) in metric_lines(game.frame_dt(), &s).iter().enumerate() {
        r.draw_text(
            crate::FONT,
            METRICS_AT + Vec2::new(0.0, LINE * i as f32),
            line.as_str(),
            &metric,
        );
    }

    let look = game.look;
    for t in TOGGLES {
        let b = t.rect();
        r.fill_rect(b, button_color(&look, game.prev_input(), t));
        let center = |dy: f32| Vec2::new(b.pos.x + b.size.x * 0.5, b.pos.y + dy);
        let centered = TextParams {
            align: TextAlign::Center,
            ..big
        };
        r.draw_text(crate::FONT, center(10.0), t.label(), &centered);
        r.draw_text(
            crate::FONT,
            center(33.0),
            look.value(t),
            &TextParams {
                tint: Color::rgb(255, 230, 120),
                ..centered
            },
        );
        // The pad button, small, in the top-right corner (clear of the
        // corner probes: 4 px in).
        r.draw_text(
            crate::FONT,
            Vec2::new(b.pos.x + b.size.x - 4.0, b.pos.y + 4.0),
            t.hint(),
            &TextParams {
                tint: Color::rgb(150, 160, 190),
                align: TextAlign::Right,
                ..TextParams::DEFAULT
            },
        );
    }
}

/// The metric lines: frame time (from the frame's `dt`), then the
/// renderer's counters. Numbers are zero-padded to a fixed width.
pub fn metric_lines(dt: f32, s: &FrameStats) -> [TextBuf<28>; 5] {
    let mut lines = [TextBuf::<28>::new(); 5];
    let tenths = ((dt * 10_000.0 + 0.5) as u32).min(999);
    let fps = if dt > 0.0 {
        ((1.0 / dt + 0.5) as u32).min(999)
    } else {
        0
    };
    let _ = write!(
        lines[0],
        "FRAME {:02}.{} MS {:03} FPS",
        tenths / 10,
        tenths % 10,
        fps
    );
    let _ = write!(
        lines[1],
        "TRIS {:05} MODELS {:03}",
        s.tris.min(99_999),
        s.models.min(999)
    );
    let _ = write!(
        lines[2],
        "SPRITES {:04} CULL {:03}",
        s.sprites.min(9_999),
        s.tnl.culled.min(999)
    );
    let _ = write!(
        lines[3],
        "VERTS {:05} CLIP {:04}",
        s.tnl.verts.min(99_999),
        s.tnl.clipped.min(9_999)
    );
    let _ = write!(
        lines[4],
        "RSP DRAWS {:03} TRIS {:05}",
        s.tnl.rsp_draws.min(999),
        s.tnl.rsp_tris.min(99_999)
    );
    lines
}

/// Read the drawn panel back (a test hook: slow, never per frame) and
/// check it against `game`'s state: each button 2 px inside its corners
/// has the fill its value calls for, 2 px outside its edges is the panel's
/// background, and the top screen does not show the panel. `Ok(false)` on
/// a single screen or a backend that cannot read back. Draw the frame
/// three times first (the 3DS LCDs are double-buffered).
pub fn probe_panel(r: &mut dyn Renderer, game: &ShowcaseGame) -> Result<bool, &'static str> {
    probe_panel_shifted(r, game, Vec2::ZERO)
}

/// [`probe_panel`] with every probe moved by `shift`: tests prove the
/// probes are tight by checking that a shifted layout fails.
pub fn probe_panel_shifted(
    r: &mut dyn Renderer,
    game: &ShowcaseGame,
    shift: Vec2,
) -> Result<bool, &'static str> {
    if r.screen_size(Screen::Bottom).is_none() {
        return Ok(false);
    }
    let close = |c: Color, want: Color| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).abs();
        d(c.r, want.r) <= 24 && d(c.g, want.g) <= 24 && d(c.b, want.b) <= 24
    };
    let px = |r: &mut dyn Renderer, screen: Screen, x: f32, y: f32| {
        let (x, y) = (x + shift.x, y + shift.y);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        r.read_screen_pixel(screen, x as u32, y as u32)
    };
    if r.read_screen_pixel(Screen::Bottom, 2, 236).is_none() {
        return Ok(false); // no read-back
    }
    for t in TOGGLES {
        let want = button_color(&game.look, game.prev_input(), t);
        let b = t.rect();
        let (x0, y0) = (b.pos.x, b.pos.y);
        let (x1, y1) = (b.pos.x + b.size.x - 1.0, b.pos.y + b.size.y - 1.0);
        let (cx, cy) = (b.pos.x + b.size.x * 0.5, b.pos.y + b.size.y * 0.5);
        for (x, y) in [
            (x0 + 2.0, y0 + 2.0),
            (x1 - 2.0, y0 + 2.0),
            (x0 + 2.0, y1 - 2.0),
            (x1 - 2.0, y1 - 2.0),
        ] {
            if !px(r, Screen::Bottom, x, y).is_some_and(|c| close(c, want)) {
                return Err("showcase panel: a button has the wrong fill (state or layout)");
            }
        }
        for (x, y) in [
            (x0 - 2.0, cy),
            (x1 + 2.0, cy),
            (cx, y0 - 2.0),
            (cx, y1 + 2.0),
        ] {
            if !px(r, Screen::Bottom, x, y).is_some_and(|c| close(c, BG)) {
                return Err("showcase panel: a button spills past its edges");
            }
        }
        // The same spot on the top screen is the daylight 3D scene, never
        // the panel's near-black background.
        let top = px(r, Screen::Top, x0 + 2.0, y0 + 2.0);
        if top.is_none_or(|c| close(c, BG)) {
            return Err("showcase panel: the panel landed on the top screen");
        }
    }
    Ok(true)
}

/// The panel end to end, for the self-tests (PC 3DS profile, Azahar): a
/// fresh game's panel probed; a top-screen tap on FOG ignored; a
/// bottom-screen tap switching the fog off, and its button turning;
/// tapped again, back to the default. The last frames drawn are the
/// default panel (the 3DS's `showcase_bottom` golden). `Ok(false)` on a
/// single screen or without read-back.
pub fn check_panel(r: &mut dyn Renderer, audio: &mut dyn Audio) -> Result<bool, &'static str> {
    let mut game = ShowcaseGame::with_screen(Vec2::new(400.0, 240.0));
    check_panel_on(&mut game, r, audio)
}

fn check_panel_on(
    game: &mut ShowcaseGame,
    r: &mut dyn Renderer,
    audio: &mut dyn Audio,
) -> Result<bool, &'static str> {
    const DT: f32 = 1.0 / 60.0;
    let draw = |game: &mut ShowcaseGame, r: &mut dyn Renderer| {
        for _ in 0..3 {
            game.render(r);
        }
    };
    let fog = Toggle::Fog.rect();
    let tap = |screen: Screen| {
        let mut s = InputState::default();
        s.pointer = Some(fog.pos + fog.size * 0.5);
        s.pointer_screen = screen.index();
        s.set_pointer_button(PointerButton::Primary, true);
        s
    };
    let idle = InputState::default();
    draw(game, r);
    if !probe_panel(r, game)? {
        return Ok(false);
    }
    game.update(&tap(Screen::Top), audio, DT);
    game.update(&idle, audio, DT);
    if game.look != Look::DEFAULT {
        return Err("showcase panel: a top-screen click toggled a panel button");
    }
    game.update(&tap(Screen::Bottom), audio, DT);
    game.update(&idle, audio, DT);
    if game.look.fog {
        return Err("showcase panel: a FOG tap did not switch the fog off");
    }
    draw(game, r);
    probe_panel(r, game)?;
    game.update(&tap(Screen::Bottom), audio, DT);
    game.update(&idle, audio, DT);
    if game.look != Look::DEFAULT {
        return Err("showcase panel: a second FOG tap did not switch it back");
    }
    draw(game, r);
    probe_panel(r, game)
}
