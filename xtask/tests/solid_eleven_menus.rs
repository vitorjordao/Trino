//! Every Solid Eleven menu, explored through its buttons: a breadth-first
//! walk from the title (and from a season that just ended) presses every
//! button on every screen state it reaches (and every club's names and
//! squad), and draws each state on the N64, the 3DS (both screens) and the
//! PC at two sizes, in every language, at 100 % and 200 % text scale,
//! through a renderer that records the 2D it is given, with the game's real
//! fonts. Each picture must:
//!
//! - draw without panicking,
//! - keep every text and tap target on its screen,
//! - have a glyph for every character it prints,
//! - not print two different texts over each other,
//! - keep every text's ink inside the box it is printed on (the innermost
//!   panel, button or row drawn before it that holds it).
//!
//! The text scale reaches the UI system's screens (`ui_screen`: they must
//! draw differently at 200 %, within each target's UI budget and with no
//! text past its box by the UI's own count) and leaves the hand-drawn menus
//! as they are (which must draw the same), see ADR 0027 §11 and issue #95.
//!
//! `cargo test -p xtask --test solid_eleven_menus -- --nocapture` prints
//! the states, the screens reached and the busiest picture.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use solid_eleven::SolidElevenGame;
use solid_eleven::game::app::{QUICK_ROWS, Scene};
use solid_eleven::sim::team::{ATTRS, Appearance};
use solid_eleven::ui::i18n::Lang;
use trino_core::text::BitmapFont;
use trino_core::{
    Button, Caps, Color, FontId, Game, InputState, Material, ModelId, ModelParams, Rect, Renderer,
    Screen, SpriteId, SpriteParams, TextAlign, TextParams, Transform3, UiStats, Vec2,
};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// A baked font: its table and, per glyph (by atlas position), the box of
/// the pixels it inks inside its cell (the cell holds the line's full
/// height; the ink is what a reader sees overflow).
struct Font {
    table: Vec<u8>,
    ink: HashMap<(u16, u16), Rect>,
}

type Fonts = HashMap<u32, Font>;

/// The game's fonts, baked by the asset pipeline once per test binary:
/// both tests baking into one directory at once let one read the index
/// while the other rewrote it (a flaky failure on macOS CI).
fn fonts() -> &'static Fonts {
    static FONTS: std::sync::OnceLock<Fonts> = std::sync::OnceLock::new();
    FONTS.get_or_init(bake_fonts)
}

fn bake_fonts() -> Fonts {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/solid-eleven/menus");
    trino_asset_pipeline::bake_all(
        &root.join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut fonts = Fonts::new();
    for f in assets.fonts {
        // The atlas is the sprite with the font's handle.
        let atlas = assets.sprites.iter().find(|s| s.id == f.id).expect("atlas");
        let font = BitmapFont::parse(&f.table).unwrap();
        let mut ink = HashMap::new();
        for g in solid_eleven::ui::FONT_CHARS
            .chars()
            .filter_map(|c| font.glyph(c))
        {
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for y in 0..g.h as u32 {
                for x in 0..g.w as u32 {
                    let (ax, ay) = (g.x as u32 + x, g.y as u32 + y);
                    if atlas.rgba[((ay * atlas.width + ax) * 4 + 3) as usize] > 96 {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                    }
                }
            }
            if x0 < x1 {
                ink.insert(
                    (g.x, g.y),
                    Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32),
                );
            }
        }
        fonts.insert(
            f.id,
            Font {
                table: f.table,
                ink,
            },
        );
    }
    fonts
}

#[derive(Clone, Debug)]
struct Text {
    screen: u8,
    /// Bumped by every fill covering most of the screen: an overlay (the
    /// pause menu's dim) starts a new layer, and texts in different
    /// layers may cover each other.
    layer: u32,
    rect: Rect,
    /// The pixels its glyphs ink (`None`: only spaces).
    ink: Option<Rect>,
    /// Fills drawn before it (an index into `Recorder::fills`).
    after: usize,
    text: String,
}

/// A fill: the boxes texts sit in. A slanted header's 2-pixel strips are
/// merged into one box (their bounds).
#[derive(Clone, Debug)]
struct Fill {
    screen: u8,
    layer: u32,
    rect: Rect,
    color: Color,
    /// The width of the strips merged into it (0: a plain fill).
    strip: f32,
    /// Where the last strip merged into it starts.
    strip_x: f32,
}

/// A renderer that keeps the 2D it draws.
struct Recorder<'a> {
    caps: Caps,
    bottom: Option<Vec2>,
    screen: u8,
    fonts: &'a Fonts,
    texts: Vec<Text>,
    fills: Vec<Fill>,
    layer: u32,
    rects: usize,
    glyphs: usize,
    missing: Vec<(char, String)>,
    /// What the UI system reported for each screen it drew this frame.
    ui: Vec<UiStats>,
}

impl<'a> Recorder<'a> {
    fn new(w: u16, h: u16, bottom: bool, fonts: &'a Fonts) -> Recorder<'a> {
        // The target's own table (its family, safe area and UI budget) at
        // the picture's size.
        let base = match (w, h) {
            (320, 240) => Caps::N64,
            (400, 240) => Caps::N3DS,
            _ => Caps::PC,
        };
        let caps = Caps {
            screen_width: w,
            screen_height: h,
            ..base
        };
        Recorder {
            caps,
            bottom: bottom.then(|| Vec2::new(320.0, 240.0)),
            screen: 0,
            fonts,
            texts: Vec::new(),
            fills: Vec::new(),
            layer: 0,
            rects: 0,
            glyphs: 0,
            missing: Vec::new(),
            ui: Vec::new(),
        }
    }

    fn size(&self, screen: u8) -> Vec2 {
        if screen == 1 {
            self.bottom.unwrap()
        } else {
            Vec2::new(
                self.caps.screen_width as f32,
                self.caps.screen_height as f32,
            )
        }
    }
}

impl Renderer for Recorder<'_> {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: Color) {
        self.screen = 0;
        self.texts.clear();
        self.fills.clear();
        self.layer = 0;
        self.rects = 0;
        self.glyphs = 0;
        self.ui.clear();
    }
    fn ui_stats(&mut self, stats: UiStats) {
        self.ui.push(stats);
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, r: Rect, color: Color) {
        self.rects += 1;
        let s = self.size(self.screen);
        if r.size.x * r.size.y > s.x * s.y * 0.5 {
            self.layer += 1;
        }
        // The next strip of a slant: same color and width, right below the
        // last one, shifted by a little (a polygon's straight sides, the
        // crests', don't shift: those spans stay apart, too thin for a box).
        if let Some(last) = self.fills.last_mut() {
            let end = last.rect.pos.y + last.rect.size.y;
            let strip = if last.strip > 0.0 {
                last.strip
            } else {
                last.rect.size.x
            };
            if last.screen == self.screen
                && last.layer == self.layer
                && last.color == color
                && r.size.y <= 2.0
                && (r.size.x - strip).abs() < 0.01
                && (r.pos.y - end).abs() < 0.01
                && (r.pos.x - last.strip_x).abs() > 0.01
                && (r.pos.x - last.strip_x).abs() < strip * 0.5
                && (last.strip > 0.0 || last.rect.size.y <= 2.0)
            {
                let x0 = last.rect.pos.x.min(r.pos.x);
                let x1 = (last.rect.pos.x + last.rect.size.x).max(r.pos.x + r.size.x);
                last.rect = Rect::new(
                    x0,
                    last.rect.pos.y,
                    x1 - x0,
                    end + r.size.y - last.rect.pos.y,
                );
                last.strip = strip;
                last.strip_x = r.pos.x;
                return;
            }
        }
        self.fills.push(Fill {
            screen: self.screen,
            layer: self.layer,
            rect: r,
            color,
            strip: 0.0,
            strip_x: r.pos.x,
        });
    }
    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, p: &TextParams) {
        let size = self.measure_text(font, text, p.scale);
        let mut ink: Option<Rect> = None;
        if let Some(fnt) = self.fonts.get(&font.0) {
            let f = BitmapFont::parse(&fnt.table).unwrap();
            for c in text.chars() {
                if c != ' ' && c != '\n' && f.glyph(c).is_none() {
                    self.missing.push((c, text.to_string()));
                }
            }
            trino_core::text::layout(&f, pos, text, p, &mut |region, at| {
                let key = (region.pos.x as u16, region.pos.y as u16);
                if let Some(g) = fnt.ink.get(&key) {
                    let r = Rect::new(
                        at.x + g.pos.x * p.scale,
                        at.y + g.pos.y * p.scale,
                        g.size.x * p.scale,
                        g.size.y * p.scale,
                    );
                    ink = Some(match ink {
                        None => r,
                        Some(u) => union(u, r),
                    });
                }
            });
        }
        let x = match p.align {
            TextAlign::Left => pos.x,
            TextAlign::Center => pos.x - size.x * 0.5,
            TextAlign::Right => pos.x - size.x,
        };
        self.glyphs += text.chars().filter(|c| *c != ' ').count();
        self.texts.push(Text {
            screen: self.screen,
            layer: self.layer,
            rect: Rect::new(x, pos.y, size.x, size.y),
            ink,
            after: self.fills.len(),
            text: text.to_string(),
        });
    }
    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        match self.fonts.get(&font.0) {
            Some(f) => BitmapFont::parse(&f.table).unwrap().measure(text, scale),
            None => Vec2::ZERO,
        }
    }
    fn second_screen(&self) -> Option<Vec2> {
        self.bottom
    }
    fn set_screen(&mut self, s: u8) {
        if self.bottom.is_some() && s >= self.screen {
            self.screen = s.min(1);
        }
    }
    fn screen(&self) -> Screen {
        if self.screen == 1 {
            Screen::Bottom
        } else {
            Screen::Top
        }
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.pos.x.min(b.pos.x);
    let y0 = a.pos.y.min(b.pos.y);
    let x1 = (a.pos.x + a.size.x).max(b.pos.x + b.size.x);
    let y1 = (a.pos.y + a.size.y).max(b.pos.y + b.size.y);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn holds(outer: Rect, p: Vec2) -> bool {
    p.x >= outer.pos.x
        && p.y >= outer.pos.y
        && p.x <= outer.pos.x + outer.size.x
        && p.y <= outer.pos.y + outer.size.y
}

/// The box `t` is printed on: the innermost fill drawn before it on its
/// screen and layer that holds the middle of its ink and is at least as
/// thick as the ink is tall both ways (a 1-unit rim or divider, a stat bar
/// with its value over it, a crest's spans are not boxes).
///
/// A fill painted over where the text is by a later candidate is not its
/// box (the toast's panel over a menu button, issue #95).
fn container<'f>(fills: &'f [Fill], t: &Text, ink: Rect) -> Option<&'f Fill> {
    let mid = Vec2::new(ink.pos.x + ink.size.x * 0.5, ink.pos.y + ink.size.y * 0.5);
    let candidates: Vec<(usize, &Fill)> = fills[..t.after]
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.screen == t.screen
                && f.layer == t.layer
                && f.color.a > 0
                && f.rect.size.y.min(f.rect.size.x) + 0.01 >= ink.size.y
                && holds(f.rect, mid)
        })
        .collect();
    let covers = |outer: Rect, r: Rect| {
        holds(outer, r.pos) && holds(outer, Vec2::new(r.pos.x + r.size.x, r.pos.y + r.size.y))
    };
    candidates
        .iter()
        .filter(|(i, _)| {
            !candidates
                .iter()
                .any(|(j, later)| j > i && covers(later.rect, ink))
        })
        .map(|(_, f)| *f)
        .min_by(|a, b| {
            let area = |f: &Fill| f.rect.size.x * f.rect.size.y;
            area(a).total_cmp(&area(b))
        })
}

/// The pictures every state is drawn at: (name, width, height, 3DS bottom).
const SCREENS: [(&str, u16, u16, bool); 4] = [
    ("n64", 320, 240, false),
    ("3ds", 400, 240, true),
    ("pc360", 640, 360, false),
    ("pc1080", 1920, 1080, false),
];

const BUTTONS: [Button; 11] = [
    Button::DpadUp,
    Button::DpadDown,
    Button::DpadLeft,
    Button::DpadRight,
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L,
    Button::R,
    Button::Start,
];

/// What makes two menu states the same for the walk.
fn key(g: &SolidElevenGame) -> String {
    format!(
        "{:?}|{:?}|{}|{}|{}|{:?}|{}",
        g.scene,
        g.cur,
        g.career_on,
        g.capture.is_some(),
        g.edit.is_some(),
        g.settings,
        g.world.round
    )
}

fn press(g: &mut SolidElevenGame, b: Button) {
    let mut s = InputState::default();
    s.set(b, true);
    g.update(&s, &mut Mute, 1.0 / 30.0);
    g.update(&InputState::default(), &mut Mute, 1.0 / 30.0);
}

/// Problems found drawing one state.
fn check(g: &SolidElevenGame, fonts: &Fonts, path: &str, report: &mut Report) {
    for &(name, w, h, bottom) in &SCREENS {
        for (lang, text_scale) in Lang::ALL
            .into_iter()
            .flat_map(|l| TEXT_SCALES.map(|t| (l, t)))
        {
            let mut g = g.clone();
            g.settings.lang = lang;
            g.access.text_scale = text_scale;
            g.resize(Vec2::new(w as f32, h as f32));
            let mut r = Recorder::new(w, h, bottom, fonts);
            let drew = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.render(&mut r)));
            let at = format!(
                "{:?} on {name} ({lang:?}, text {text_scale} %) via {path}",
                g.scene
            );
            if drew.is_err() {
                report.problems.push(format!("panic drawing {at}"));
                continue;
            }
            report.pictures += 1;
            // Text scale reaches the UI system's screens (issue #95) and
            // leaves the hand-drawn ones as they are.
            // (The toast is a UI screen over any menu: it grows anywhere.)
            let toast_text = g
                .toast
                .map(|t| solid_eleven::ui::i18n::tr(lang, t.text).to_string());
            // (The 3DS touch screen is a UI screen in every scene: only the
            // top screen can be hand-drawn.)
            let texts: Vec<(String, Rect)> = r
                .texts
                .iter()
                .filter(|t| t.screen == 0)
                .filter(|t| toast_text.as_deref() != Some(t.text.as_str()))
                .map(|t| (t.text.clone(), t.rect))
                .collect();
            let key = (name, lang);
            let scene = scene_name(g.scene);
            if text_scale == TEXT_SCALES[0] {
                report.at_100.insert(key, texts);
            } else if let Some(base) = report.at_100.get(&key) {
                let changed = base != &texts;
                if ui_screen(&g) {
                    if changed {
                        report.grew.insert(scene.clone());
                    }
                } else if changed {
                    report.problems.push(format!(
                        "a hand-drawn screen changed with the text scale: {at}"
                    ));
                }
            }
            // The UI system's own report: within the target's budget, every
            // node laid out, no text past its box.
            let caps = *r.caps();
            for st in &r.ui {
                if st.nodes > caps.max_ui_nodes || st.draws > caps.max_ui_draws {
                    report.problems.push(format!(
                        "over the UI budget ({} nodes of {}, {} draws of {}): {at}",
                        st.nodes, caps.max_ui_nodes, st.draws, caps.max_ui_draws
                    ));
                }
                if st.dropped > 0 {
                    report
                        .problems
                        .push(format!("{} UI nodes not laid out: {at}", st.dropped));
                }
                if st.overflow > 0 {
                    // Which nodes (index and list item) in the menu screen.
                    let nodes: Vec<String> = g
                        .menu_ui
                        .slots()
                        .iter()
                        .filter(|s| s.has(trino_core::ui::flags::OVERFLOW))
                        .map(|s| format!("#{}/{}", s.node, s.item))
                        .collect();
                    report.problems.push(format!(
                        "{} UI texts past their box {nodes:?}: {at}",
                        st.overflow
                    ));
                }
            }
            for (c, text) in &r.missing {
                report
                    .problems
                    .push(format!("no glyph for {c:?} in {text:?}: {at}"));
            }
            for t in &r.texts {
                let s = r.size(t.screen);
                let rc = t.rect;
                if rc.pos.x < -0.5
                    || rc.pos.y < -0.5
                    || rc.pos.x + rc.size.x > s.x + 0.5
                    || rc.pos.y + rc.size.y > s.y + 0.5
                {
                    report.problems.push(format!(
                        "{:?} off screen {} at {:?}: {at}",
                        t.text, t.screen, rc
                    ));
                }
            }
            // Ink outside the box it is printed on.
            for t in &r.texts {
                let Some(ink) = t.ink else { continue };
                let Some(b) = container(&r.fills, t, ink) else {
                    continue;
                };
                let o = b.rect;
                let out = (o.pos.x - ink.pos.x)
                    .max(o.pos.y - ink.pos.y)
                    .max(ink.pos.x + ink.size.x - (o.pos.x + o.size.x))
                    .max(ink.pos.y + ink.size.y - (o.pos.y + o.size.y));
                if out > 0.5 {
                    report.problems.push(format!(
                        "{:?} spills {out:.0} px out of its box {o:?} (ink {ink:?}): {at}",
                        t.text
                    ));
                }
            }
            // Two different texts over each other (a shadow is the same
            // text 1-2 px off, and doesn't count; the toast floats over the
            // screen on its own panel, an overlay starts a new layer).
            let toast = g
                .toast
                .map(|t| solid_eleven::ui::i18n::tr(lang, t.text).to_string());
            for (i, a) in r.texts.iter().enumerate() {
                for b in &r.texts[i + 1..] {
                    if a.screen != b.screen
                        || a.layer != b.layer
                        || a.text == b.text
                        || a.text.trim().is_empty()
                        || b.text.trim().is_empty()
                        || toast.as_deref() == Some(a.text.as_str())
                        || toast.as_deref() == Some(b.text.as_str())
                    {
                        continue;
                    }
                    // What they ink (two lines of one name touch only in
                    // their line boxes).
                    let (ra, rb) = (a.ink.unwrap_or(a.rect), b.ink.unwrap_or(b.rect));
                    let ox =
                        (ra.pos.x + ra.size.x).min(rb.pos.x + rb.size.x) - ra.pos.x.max(rb.pos.x);
                    let oy =
                        (ra.pos.y + ra.size.y).min(rb.pos.y + rb.size.y) - ra.pos.y.max(rb.pos.y);
                    // Overlapping by more than a pixel row of each other's
                    // glyphs, both ways (scaled with the picture).
                    let k = (h as f32 / 240.0).floor().max(1.0);
                    if ox > 2.0 * k && oy > 3.0 * k {
                        report
                            .problems
                            .push(format!("{:?} overlaps {:?}: {at}", a.text, b.text));
                    }
                }
            }
            for hit in g.hits.iter() {
                let s = r.size(if hit.screen == Screen::Bottom { 1 } else { 0 });
                let rc = hit.rect;
                if rc.pos.x < -0.5
                    || rc.pos.y < -0.5
                    || rc.pos.x + rc.size.x > s.x + 0.5
                    || rc.pos.y + rc.size.y > s.y + 0.5
                {
                    report
                        .problems
                        .push(format!("tap target {} off screen at {rc:?}: {at}", hit.id));
                }
            }
            if name == "n64" {
                let load = r.rects + r.glyphs;
                if load > report.busiest.0 {
                    report.busiest = (load, at.clone());
                }
            }
        }
    }
}

/// The text scales every state is drawn at (`AccessSettings::text_scale`).
const TEXT_SCALES: [u8; 2] = [100, 200];

/// A scene's name without its fields.
fn scene_name(s: Scene) -> String {
    format!("{s:?}").split(' ').next().unwrap().to_string()
}

/// Every screen and the match HUD now scale with the text scale (issue #95)
/// while keeping every text inside its box and on screen at 200 %.
fn ui_screen(_g: &SolidElevenGame) -> bool {
    true
}

#[derive(Default)]
struct Report {
    /// The texts of the state being checked at 100 %, per picture.
    at_100: HashMap<(&'static str, Lang), Vec<(String, Rect)>>,
    /// The UI screens that drew differently at 200 % (the text scale
    /// reached them).
    grew: HashSet<String>,
    states: usize,
    pictures: usize,
    problems: Vec<String>,
    scenes: HashSet<String>,
    busiest: (usize, String),
}

/// Breadth-first from `start`, up to `limit` states.
fn walk(start: SolidElevenGame, fonts: &Fonts, limit: usize, report: &mut Report) {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    seen.insert(key(&start));
    queue.push_back((start, String::from("start")));
    while let Some((g, path)) = queue.pop_front() {
        report.states += 1;
        report.scenes.insert(
            format!("{:?}", g.scene)
                .split(' ')
                .next()
                .unwrap()
                .to_string(),
        );
        check(&g, fonts, &path, report);
        if report.states >= limit {
            break;
        }
        // Matches: pause and leave them, don't play them (the e2e and the
        // scenarios do).
        if g.scene == Scene::Match && path.matches("Start").count() > 1 {
            continue;
        }
        for b in BUTTONS {
            let mut next = g.clone();
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| press(&mut next, b)));
            if ok.is_err() {
                report
                    .problems
                    .push(format!("panic pressing {b:?} after {path}"));
                continue;
            }
            if seen.insert(key(&next)) {
                queue.push_back((next, format!("{path} {b:?}")));
            }
        }
    }
}

#[test]
fn every_menu_state_draws_cleanly_in_every_language_and_size() {
    let fonts = fonts();
    let mut report = Report::default();
    // From the title, fresh.
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    for _ in 0..4 {
        g.update(&InputState::default(), &mut Mute, 1.0 / 30.0);
    }
    walk(g.clone(), fonts, 1_500, &mut report);
    // A career whose season just ended: the trophy, the new season.
    let mut c = g.clone();
    c.world = solid_eleven::sim::league::Career::new(2, 9);
    c.career_on = true;
    while (c.world.round as usize) < solid_eleven::sim::league::ROUNDS {
        c.world.finish_round(None);
    }
    c.scene = Scene::Trophy;
    walk(c, fonts, 300, &mut report);
    // Every club's names and squad, which the walk's key leaves out (it
    // doesn't tell clubs apart): the title's card and the quick match with
    // each club, and a career as each club, walked from its club screen.
    for club in 0..solid_eleven::sim::team::TEAMS as u8 {
        let mut t = g.clone();
        t.quick.home = club;
        t.quick.away = (club + 5) % solid_eleven::sim::team::TEAMS as u8;
        for scene in [Scene::Title, Scene::Quick] {
            t.scene = scene;
            report.states += 1;
            check(
                &t,
                fonts,
                &format!("{scene:?} with club {club}"),
                &mut report,
            );
        }
        let mut c = g.clone();
        c.world = solid_eleven::sim::league::Career::new(club, 9);
        c.career_on = true;
        c.scene = Scene::Title;
        report.states += 1;
        check(
            &c,
            fonts,
            &format!("title in a career as club {club}"),
            &mut report,
        );
        c.scene = Scene::Club;
        walk(c, fonts, 200, &mut report);
    }
    // Values the walk's key leaves out, each with its row selected: every
    // hair style and color in the player editor, every quick match option.
    let mut e = g.clone();
    e.scene = Scene::Edit;
    let i = 5;
    e.edit = Some((i, e.world.players[i]));
    for style in 0..Appearance::STYLES {
        for hair in 0..Appearance::HAIRS {
            for row in [ATTRS, ATTRS + 1, ATTRS + 2] {
                e.world.players[i].set_appearance(Appearance {
                    skin: style % Appearance::SKINS,
                    hair,
                    style,
                });
                e.cur.edit_attr = row;
                report.states += 1;
                check(
                    &e,
                    fonts,
                    &format!("edit: style {style}, hair {hair}, row {row}"),
                    &mut report,
                );
            }
        }
    }
    // The match's own menus, which the walk reaches only by playing: the
    // half-time break and the substitutions with the cursor on each pane
    // and a player picked to go off.
    let mut h = g.clone();
    h.start_exhibition(0, 1, 7, [true, false], solid_eleven::render::Sky::Day, 60);
    for _ in 0..30 {
        h.update(&InputState::default(), &mut Mute, 1.0 / 30.0);
    }
    for (scene, pane, locked) in [
        (Scene::HalfTime, 0, None),
        (
            Scene::Substitutions {
                from_half_time: true,
            },
            0,
            Some(3),
        ),
        (
            Scene::Substitutions {
                from_half_time: true,
            },
            1,
            None,
        ),
    ] {
        h.scene = scene;
        h.cur.sub_pane = pane;
        h.cur.sub_bench = 2;
        h.cur.sub_selected_pitch = locked;
        report.states += 1;
        check(&h, fonts, &format!("{scene:?} pane {pane}"), &mut report);
    }
    let mut q = g.clone();
    q.scene = Scene::Quick;
    for stadium in 0..3 {
        for sky in 0..3 {
            for row in 0..QUICK_ROWS {
                q.quick.stadium = stadium;
                q.quick.sky = sky;
                q.quick.rain = row % 2 == 0;
                q.settings.difficulty = (stadium + sky) % 3;
                q.cur.quick = row;
                report.states += 1;
                check(
                    &q,
                    fonts,
                    &format!("quick: stadium {stadium}, sky {sky}, row {row}"),
                    &mut report,
                );
            }
        }
    }
    // Every gamepad family in the controls screen (match tab and gamepad
    // tab) and the title screen (PlayStation symbol spans, Nintendo swapped
    // letters, 8BitDo, Xbox, Generic).
    let mut ctl = g.clone();
    ctl.scene = Scene::Controls;
    ctl.using_pad = true;
    ctl.peek.rebindable = true;
    ctl.peek.gamepad = 2;
    for fam in trino_core::PadFamily::ALL {
        ctl.peek.pad_family = Some(fam);
        for tab in [0u8, 2] {
            ctl.cur.ctl_tab = tab;
            report.states += 1;
            check(
                &ctl,
                fonts,
                &format!("controls: {fam:?} tab {tab}"),
                &mut report,
            );
        }
    }
    println!(
        "{} menu states, {} pictures checked; screens reached: {:?}",
        report.states,
        report.pictures,
        {
            let mut v: Vec<_> = report.scenes.iter().cloned().collect();
            v.sort();
            v
        }
    );
    println!(
        "busiest N64 picture: {} rects + glyphs ({})",
        report.busiest.0, report.busiest.1
    );
    for want in [
        "Pause",
        "Settings",
        "Controls",
        "Table",
        "Confirm",
        "Quick",
        "Results",
        "Club",
        "PickClub",
        "Teams",
        "Edit",
        "Market",
        "HalfTime",
        "Substitutions",
        "Title",
        "Trophy",
        "Match",
    ] {
        assert!(
            report.grew.contains(want),
            "{want} (a UI screen) drew the same at every text scale"
        );
    }
    for want in [
        "Title", "Quick", "PickClub", "Club", "Table", "Teams", "Edit", "Market", "Settings",
        "Controls", "Confirm", "Match", "Pause", "Trophy",
    ] {
        assert!(report.scenes.contains(want), "never reached {want}");
    }
    // One line per distinct problem (the path that found it first).
    let mut first: Vec<(String, String)> = Vec::new();
    for p in &report.problems {
        let (what, via) = p.split_once(" via ").unwrap_or((p.as_str(), ""));
        if !first.iter().any(|(w, _)| w == what) {
            first.push((what.to_string(), via.to_string()));
        }
    }
    let problems: Vec<String> = first
        .iter()
        .map(|(w, v)| format!("{w} (via {v})"))
        .collect();
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems
            .iter()
            .take(400)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Random play: buttons, the stick, triggers and taps on both screens,
/// for a long stretch of menus and matches (deterministic: a fixed LCG).
/// Nothing panics, every frame draws, and the walk visits matches too.
#[test]
fn random_input_never_breaks_the_game() {
    let fonts = fonts();
    let mut seed = 0x2545_f491_u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed >> 8
    };
    let mut g = SolidElevenGame::new(Vec2::new(400.0, 240.0));
    let mut r = Recorder::new(400, 240, true, fonts);
    let mut scenes = HashSet::new();
    let mut prev = InputState::default();
    for f in 0..30_000u32 {
        let mut s = InputState::default();
        // A button now and then, held for a few frames.
        if rnd() % 6 == 0 {
            let b = BUTTONS[(rnd() as usize) % BUTTONS.len()];
            s.set(b, true);
        }
        if rnd() % 4 == 0 {
            s.stick = Vec2::new(
                (rnd() % 200) as f32 / 100.0 - 1.0,
                (rnd() % 200) as f32 / 100.0 - 1.0,
            );
        }
        if rnd() % 50 == 0 {
            s.triggers = Vec2::new(0.0, 1.0);
        }
        // A tap somewhere on one of the screens.
        if rnd() % 40 == 0 {
            let bottom = rnd() % 2 == 0;
            let (w, h) = if bottom {
                (320.0, 240.0)
            } else {
                (400.0, 240.0)
            };
            s.pointer = Some(Vec2::new(
                (rnd() % w as u32) as f32,
                (rnd() % h as u32) as f32,
            ));
            s.pointer_screen = bottom as u8;
            s.set_pointer_button(trino_core::PointerButton::Primary, true);
        }
        // Don't let a menu loop forever: back to the title once in a while.
        if f % 4_000 == 3_999 {
            g.scene = Scene::Title;
            g.session = None;
        }
        g.update(&s, &mut Mute, 1.0 / 30.0);
        if f % 10 == 0 {
            g.render(&mut r);
        }
        scenes.insert(
            format!("{:?}", g.scene)
                .split(' ')
                .next()
                .unwrap()
                .to_string(),
        );
        prev = s;
    }
    let _ = prev;
    println!("random play visited: {scenes:?}");
    assert!(scenes.contains("Match"), "{scenes:?}");
    assert!(
        r.missing.is_empty(),
        "{:?}",
        &r.missing[..r.missing.len().min(5)]
    );
}
