//! Every Solid Eleven menu, explored through its buttons: a breadth-first
//! walk from the title (and from a season that just ended) presses every
//! button on every screen state it reaches, and draws each state on the
//! N64, the 3DS (both screens) and the PC at two sizes, in English and
//! Portuguese, through a renderer that records the 2D it is given, with
//! the game's real fonts. Each picture must:
//!
//! - draw without panicking,
//! - keep every text and tap target on its screen,
//! - have a glyph for every character it prints,
//! - not print two different texts over each other.
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
    Screen, SpriteId, SpriteParams, TextAlign, TextParams, Transform3, Vec2,
};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// The game's fonts, baked by the asset pipeline once per test binary:
/// both tests baking into one directory at once let one read the index
/// while the other rewrote it (a flaky failure on macOS CI).
fn fonts() -> HashMap<u32, Vec<u8>> {
    static FONTS: std::sync::OnceLock<HashMap<u32, Vec<u8>>> = std::sync::OnceLock::new();
    FONTS.get_or_init(bake_fonts).clone()
}

fn bake_fonts() -> HashMap<u32, Vec<u8>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/solid-eleven/menus");
    trino_asset_pipeline::bake_all(
        &root.join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    assets.fonts.into_iter().map(|f| (f.id, f.table)).collect()
}

#[derive(Clone, Debug)]
struct Text {
    screen: u8,
    /// Bumped by every fill covering most of the screen: an overlay (the
    /// pause menu's dim) starts a new layer, and texts in different
    /// layers may cover each other.
    layer: u32,
    rect: Rect,
    text: String,
}

/// A renderer that keeps the 2D it draws.
struct Recorder<'a> {
    caps: Caps,
    bottom: Option<Vec2>,
    screen: u8,
    fonts: &'a HashMap<u32, Vec<u8>>,
    texts: Vec<Text>,
    layer: u32,
    rects: usize,
    glyphs: usize,
    missing: Vec<(char, String)>,
}

impl<'a> Recorder<'a> {
    fn new(w: u16, h: u16, bottom: bool, fonts: &'a HashMap<u32, Vec<u8>>) -> Recorder<'a> {
        let caps = Caps {
            screen_width: w,
            screen_height: h,
            ..Caps::N64
        };
        Recorder {
            caps,
            bottom: bottom.then(|| Vec2::new(320.0, 240.0)),
            screen: 0,
            fonts,
            texts: Vec::new(),
            layer: 0,
            rects: 0,
            glyphs: 0,
            missing: Vec::new(),
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
        self.layer = 0;
        self.rects = 0;
        self.glyphs = 0;
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, r: Rect, _: Color) {
        self.rects += 1;
        let s = self.size(self.screen);
        if r.size.x * r.size.y > s.x * s.y * 0.5 {
            self.layer += 1;
        }
    }
    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, p: &TextParams) {
        let size = self.measure_text(font, text, p.scale);
        if let Some(bytes) = self.fonts.get(&font.0) {
            let f = BitmapFont::parse(bytes).unwrap();
            for c in text.chars() {
                if c != ' ' && c != '\n' && f.glyph(c).is_none() {
                    self.missing.push((c, text.to_string()));
                }
            }
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
            text: text.to_string(),
        });
    }
    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        match self.fonts.get(&font.0) {
            Some(bytes) => BitmapFont::parse(bytes).unwrap().measure(text, scale),
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
fn check(g: &SolidElevenGame, fonts: &HashMap<u32, Vec<u8>>, path: &str, report: &mut Report) {
    for &(name, w, h, bottom) in &SCREENS {
        for lang in [Lang::En, Lang::Pt] {
            let mut g = g.clone();
            g.settings.lang = lang;
            g.resize(Vec2::new(w as f32, h as f32));
            let mut r = Recorder::new(w, h, bottom, fonts);
            let drew = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.render(&mut r)));
            let at = format!("{:?} on {name} ({lang:?}) via {path}", g.scene);
            if drew.is_err() {
                report.problems.push(format!("panic drawing {at}"));
                continue;
            }
            report.pictures += 1;
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
                    let ox = (a.rect.pos.x + a.rect.size.x).min(b.rect.pos.x + b.rect.size.x)
                        - a.rect.pos.x.max(b.rect.pos.x);
                    let oy = (a.rect.pos.y + a.rect.size.y).min(b.rect.pos.y + b.rect.size.y)
                        - a.rect.pos.y.max(b.rect.pos.y);
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

#[derive(Default)]
struct Report {
    states: usize,
    pictures: usize,
    problems: Vec<String>,
    scenes: HashSet<String>,
    busiest: (usize, String),
}

/// Breadth-first from `start`, up to `limit` states.
fn walk(start: SolidElevenGame, fonts: &HashMap<u32, Vec<u8>>, limit: usize, report: &mut Report) {
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
    walk(g.clone(), &fonts, 1_500, &mut report);
    // A career whose season just ended: the trophy, the new season.
    let mut c = g.clone();
    c.world = solid_eleven::sim::league::Career::new(2, 9);
    c.career_on = true;
    while (c.world.round as usize) < solid_eleven::sim::league::ROUNDS {
        c.world.finish_round(None);
    }
    c.scene = Scene::Trophy;
    walk(c, &fonts, 300, &mut report);
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
                    &fonts,
                    &format!("edit: style {style}, hair {hair}, row {row}"),
                    &mut report,
                );
            }
        }
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
                    &fonts,
                    &format!("quick: stadium {stadium}, sky {sky}, row {row}"),
                    &mut report,
                );
            }
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
            .take(120)
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
    let mut r = Recorder::new(400, 240, true, &fonts);
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
