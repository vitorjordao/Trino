//! Accessibility (issue #60) through the public API: the `TACC` blob and
//! its lookups (semantic colors per color-vision mode, captions, icons, the
//! high-contrast theme), the caption queue and the audio wrapper, camera
//! effects under reduced motion, the flash guard and the strobe monitor,
//! hold-to-toggle, and the UI's text scale, high contrast and icons.

mod common;

use common::MemoryPlatform;
use trino_core::access::data::{self, CaptionInfo, CaptionRecord};
use trino_core::access::{CaptionMode, MAX_FLASHES_PER_SECOND};
use trino_core::ui::{
    Anchor, CvdIcon, NONE, NoStrings, NodeKind, Nodes, Scaling, Size, Theme, UiAccess, UiNode,
    UiStrings,
};
use trino_core::{
    AccessBlob, AccessData, AccessSettings, Audio, AudioStats, BlobId, Bus, Button, CameraFx, Caps,
    CaptionPlace, CaptionStyle, Captions, Color, CvdMode, DuckRule, FlashMonitor, FontId,
    HoldToggle, InputState, MusicId, Rect, Renderer, Screen, ScreenFlash, SoundId, SoundParams,
    SpriteId, SpriteParams, TextParams, Ui, UiEnv, UiScreen, Vec2, Vec3, VoiceId, asset_id,
};

// ------------------------------------------------------------- the blob ----

const RED: Color = Color::rgb(220, 40, 40);
const ORANGE: Color = Color::rgb(240, 160, 0);
const GREEN: Color = Color::rgb(40, 160, 60);
const BLUE: Color = Color::rgb(40, 90, 220);

fn sample() -> Vec<u8> {
    let mut colors = vec![
        (
            asset_id("team.a"),
            [RED, RED, ORANGE, RED, Color::rgb(90, 90, 90)],
        ),
        (
            asset_id("team.b"),
            [GREEN, BLUE, BLUE, GREEN, Color::rgb(20, 20, 20)],
        ),
    ];
    colors.sort_by_key(|c| c.0);
    let mut captions = vec![
        CaptionRecord {
            sound: asset_id("sounds/whistle"),
            info: CaptionInfo {
                key: "cap.whistle",
                priority: 200,
                important: true,
                direction: false,
                duration_ms: 1500,
            },
        },
        CaptionRecord {
            sound: asset_id("sounds/step"),
            info: CaptionInfo {
                key: "cap.step",
                priority: 0,
                important: false,
                direction: true,
                duration_ms: 0,
            },
        },
    ];
    captions.sort_by_key(|c| c.sound);
    let icons = [(asset_id("sprites/kit"), asset_id("sprites/stripes"))];
    let theme = Theme {
        focus: Color::rgb(0, 200, 255),
        ..Theme::HIGH_CONTRAST
    };
    let mut out = vec![0u8; data::encoded_len(colors.len(), &captions, icons.len())];
    let n = data::encode(&colors, &captions, &icons, Some(&theme), &mut out).unwrap();
    assert_eq!(n, out.len());
    out
}

#[test]
fn tacc_round_trips_and_answers_per_mode() {
    let bytes = sample();
    let d = AccessData::parse(&bytes).unwrap();
    assert_eq!(
        (d.color_count(), d.caption_count(), d.icon_count()),
        (2, 2, 1)
    );
    assert_eq!(d.color("team.a", CvdMode::Off), Some(RED));
    assert_eq!(d.color("team.a", CvdMode::Deuteranopia), Some(ORANGE));
    assert_eq!(d.color("team.b", CvdMode::Protanopia), Some(BLUE));
    assert_eq!(d.color("team.c", CvdMode::Off), None);
    // A palette for a slotted mesh: empty or unknown names keep white (the
    // baked color).
    assert_eq!(
        d.palette(["team.a", "", "nope", "team.b"], CvdMode::Deuteranopia),
        [ORANGE, Color::WHITE, Color::WHITE, BLUE]
    );
    let w = d.caption(asset_id("sounds/whistle")).unwrap();
    assert_eq!((w.key, w.priority, w.important), ("cap.whistle", 200, true));
    assert_eq!(w.duration_ms, 1500);
    assert!(d.caption(asset_id("sounds/step")).unwrap().direction);
    assert_eq!(d.caption(asset_id("sounds/none")), None);
    assert_eq!(
        d.icon(SpriteId::from_path("sprites/kit")),
        Some(SpriteId::from_path("sprites/stripes"))
    );
    assert_eq!(d.icon(SpriteId::from_path("sprites/stripes")), None);
    assert_eq!(d.contrast_theme().unwrap().focus, Color::rgb(0, 200, 255));
    // The empty table answers nothing.
    assert_eq!(AccessData::EMPTY.color("team.a", CvdMode::Off), None);
    assert_eq!(AccessData::EMPTY.contrast_theme(), None);
}

#[test]
fn damaged_tacc_blobs_are_refused() {
    use trino_core::access::AccessError;
    let good = sample();
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(AccessData::parse(&bad).err(), Some(AccessError::BadMagic));
    let mut bad = good.clone();
    bad[4] = 9;
    assert_eq!(
        AccessData::parse(&bad).err(),
        Some(AccessError::UnsupportedVersion(9))
    );
    assert_eq!(
        AccessData::parse(&good[..data::HEADER + 10]).err(),
        Some(AccessError::Truncated)
    );
    // Swap the two colors' ids: out of order.
    let mut bad = good.clone();
    let (a, b) = (data::HEADER, data::HEADER + data::COLOR_RECORD);
    let ida: [u8; 4] = bad[a..a + 4].try_into().unwrap();
    let idb: [u8; 4] = bad[b..b + 4].try_into().unwrap();
    bad[a..a + 4].copy_from_slice(&idb);
    bad[b..b + 4].copy_from_slice(&ida);
    assert_eq!(AccessData::parse(&bad).err(), Some(AccessError::Unsorted));
    // A caption key past the pool.
    let mut bad = good.clone();
    let k = data::HEADER + 2 * data::COLOR_RECORD + 6;
    bad[k] = 0xff;
    assert_eq!(AccessData::parse(&bad).err(), Some(AccessError::BadString));
    // The writer refuses unsorted tables and a short buffer.
    let colors = [(2, [RED; 5]), (1, [RED; 5])];
    let mut out = vec![0u8; 256];
    assert_eq!(
        data::encode(&colors, &[], &[], None, &mut out),
        Err(AccessError::Unsorted)
    );
    assert_eq!(
        data::encode(&colors[..1], &[], &[], None, &mut out[..10]),
        Err(AccessError::NoRoom)
    );
}

#[test]
fn the_access_blob_follows_live_reloads() {
    let mut p = MemoryPlatform::new(512);
    let mut blob = AccessBlob::new();
    assert!(blob.get(&p).is_none(), "no blob baked");
    p.insert_blob(BlobId::from_path(data::LOGICAL), sample());
    assert_eq!(
        blob.get(&p).unwrap().color("team.a", CvdMode::Off),
        Some(RED)
    );
    // A live edit hands out new bytes: parsed again.
    let mut next = sample();
    let at = data::HEADER + 4;
    next[at] = 1;
    // The first record (the smaller id) has its base red byte at +4.
    let which = if asset_id("team.a") < asset_id("team.b") {
        "team.a"
    } else {
        "team.b"
    };
    p.insert_blob(BlobId::from_path(data::LOGICAL), next);
    assert_eq!(
        blob.get(&p).unwrap().color(which, CvdMode::Off).unwrap().r,
        1
    );
    // A broken blob is reported, not a crash.
    p.insert_blob(BlobId::from_path(data::LOGICAL), b"TACC".to_vec());
    assert!(blob.get(&p).is_none());
    assert!(blob.error.is_some());
}

// ------------------------------------------------------------- captions ----

/// Every `Audio` call, counted, so the wrapper is proven to forward all.
#[derive(Default)]
struct Spy {
    calls: Vec<&'static str>,
}

impl Audio for Spy {
    fn play_sound(&mut self, _: SoundId) {
        self.calls.push("play_sound");
    }
    fn play_music(&mut self, _: MusicId, _: bool) {
        self.calls.push("play_music");
    }
    fn stop_music(&mut self) {
        self.calls.push("stop_music");
    }
    fn set_master_volume(&mut self, _: f32) {
        self.calls.push("set_master_volume");
    }
    fn play_sound_with(&mut self, _: SoundId, _: &SoundParams) {
        self.calls.push("play_sound_with");
    }
    fn play_voice(&mut self, _: SoundId, _: &SoundParams, _: bool) -> Option<VoiceId> {
        self.calls.push("play_voice");
        Some(VoiceId(7))
    }
    fn set_voice(&mut self, _: VoiceId, _: &SoundParams) {
        self.calls.push("set_voice");
    }
    fn stop_voice(&mut self, _: VoiceId) {
        self.calls.push("stop_voice");
    }
    fn voice_playing(&self, v: VoiceId) -> bool {
        v == VoiceId(7)
    }
    fn play_stinger(&mut self, _: MusicId) {
        self.calls.push("play_stinger");
    }
    fn stinger_playing(&self) -> bool {
        true
    }
    fn set_music_volume(&mut self, _: f32) {
        self.calls.push("set_music_volume");
    }
    fn set_bus_volume(&mut self, _: Bus, _: f32) {
        self.calls.push("set_bus_volume");
    }
    fn bus_volume(&self, _: Bus) -> f32 {
        0.25
    }
    fn set_bus_muted(&mut self, _: Bus, _: bool) {
        self.calls.push("set_bus_muted");
    }
    fn bus_muted(&self, _: Bus) -> bool {
        true
    }
    fn set_duck(&mut self, _: DuckRule) {
        self.calls.push("set_duck");
    }
    fn clear_ducks(&mut self) {
        self.calls.push("clear_ducks");
    }
    fn bus_duck(&self, _: Bus) -> f32 {
        0.5
    }
    fn audio_stats(&self) -> AudioStats {
        AudioStats {
            voices: 3,
            ..AudioStats::default()
        }
    }
}

fn settings(captions: CaptionMode) -> AccessSettings {
    AccessSettings {
        captions,
        ..AccessSettings::DEFAULT
    }
}

#[test]
fn the_caption_wrapper_forwards_every_call_and_captions_what_plays() {
    let bytes = sample();
    let d = AccessData::parse(&bytes).unwrap();
    let data: &'static AccessData<'static> = Box::leak(Box::new(
        AccessData::parse(Box::leak(bytes.clone().into_boxed_slice())).unwrap(),
    ));
    let _ = d;
    let mut caps = Captions::new();
    caps.configure(Some(data), &settings(CaptionMode::All));
    let mut spy = Spy::default();
    let whistle = SoundId::from_path("sounds/whistle");
    let step = SoundId::from_path("sounds/step");
    {
        let mut a = caps.wrap(&mut spy);
        a.play_sound(whistle);
        a.play_sound_with(
            step,
            &SoundParams {
                pan: -0.8,
                ..SoundParams::DEFAULT
            },
        );
        assert_eq!(
            a.play_voice(step, &SoundParams::DEFAULT, true),
            Some(VoiceId(7))
        );
        a.set_voice(VoiceId(7), &SoundParams::DEFAULT);
        assert!(a.voice_playing(VoiceId(7)));
        a.stop_voice(VoiceId(7));
        a.play_music(MusicId::from_path("music/x"), true);
        a.stop_music();
        a.play_stinger(MusicId::from_path("music/y"));
        assert!(a.stinger_playing());
        a.set_master_volume(1.0);
        a.set_music_volume(1.0);
        a.set_bus_volume(Bus::Sfx, 0.5);
        assert_eq!(a.bus_volume(Bus::Sfx), 0.25);
        a.set_bus_muted(Bus::Sfx, true);
        assert!(a.bus_muted(Bus::Sfx));
        a.set_duck(DuckRule {
            trigger: Bus::Voice,
            target: Bus::Music,
            level: 0.5,
            attack_ms: 10,
            release_ms: 10,
        });
        a.clear_ducks();
        assert_eq!(a.bus_duck(Bus::Music), 0.5);
        assert_eq!(a.audio_stats().voices, 3);
    }
    assert_eq!(
        spy.calls,
        [
            "play_sound",
            "play_sound_with",
            "play_voice",
            "set_voice",
            "stop_voice",
            "play_music",
            "stop_music",
            "play_stinger",
            "set_master_volume",
            "set_music_volume",
            "set_bus_volume",
            "set_bus_muted",
            "set_duck",
            "clear_ducks",
        ]
    );
    // The whistle and the step (twice: a one-shot and a voice) are
    // captioned; the music has none. The voice came last, centered: no
    // arrow.
    assert_eq!(caps.keys().collect::<Vec<_>>(), ["cap.whistle", "cap.step"]);
    let texts = caps.texts(&NoStrings);
    assert_eq!(texts[0].as_ref().map(|t| t.as_str()), Some("cap.whistle"));
    assert_eq!(texts[1].as_ref().map(|t| t.as_str()), Some("cap.step (x2)"));
}

#[test]
fn captions_follow_the_mode_priority_and_time() {
    let data: &'static AccessData<'static> = Box::leak(Box::new(
        AccessData::parse(Box::leak(sample().into_boxed_slice())).unwrap(),
    ));
    let whistle = asset_id("sounds/whistle");
    let step = asset_id("sounds/step");
    let mut caps = Captions::new();
    // Off: nothing; Important: only the whistle.
    caps.configure(Some(data), &settings(CaptionMode::Off));
    assert!(!caps.on_sound(whistle, None));
    caps.configure(Some(data), &settings(CaptionMode::Important));
    assert!(!caps.on_sound(step, None));
    assert!(caps.on_sound(whistle, None));
    // Its own duration (1.5 s), then gone.
    caps.update(1.4);
    assert_eq!(caps.len(), 1);
    caps.update(0.2);
    assert!(caps.is_empty());
    // A full queue drops the lowest priority, never for a lower one.
    caps.configure(Some(data), &settings(CaptionMode::All));
    let low = |key: &'static str, priority| CaptionInfo {
        key,
        priority,
        important: false,
        direction: false,
        duration_ms: 0,
    };
    for k in ["a", "b", "c", "d"] {
        assert!(caps.push(low(k, 10), None));
    }
    assert!(
        !caps.push(low("e", 5), None),
        "lower than everything queued"
    );
    assert!(caps.on_sound(whistle, None));
    assert_eq!(
        caps.keys().collect::<Vec<_>>(),
        ["b", "c", "d", "cap.whistle"]
    );
    // Turning captions off clears them.
    caps.configure(Some(data), &settings(CaptionMode::Off));
    assert!(caps.is_empty());
    // A strings table gives the texts; a pan right points right.
    struct Pt;
    impl UiStrings for Pt {
        fn get(&self, key: &str) -> Option<&str> {
            (key == "cap.step").then_some("[passos]")
        }
    }
    caps.configure(Some(data), &settings(CaptionMode::All));
    caps.on_sound(step, Some(0.9));
    assert_eq!(
        caps.texts(&Pt)[0].as_ref().map(|t| t.as_str()),
        Some("[passos] >")
    );
    caps.clear();
    caps.on_sound(step, Some(-0.9));
    assert_eq!(
        caps.texts(&Pt)[0].as_ref().map(|t| t.as_str()),
        Some("< [passos]")
    );
    // A sound without `direction` never points.
    caps.on_sound(asset_id("sounds/whistle"), Some(-0.9));
    assert_eq!(
        caps.texts(&Pt)[1].as_ref().map(|t| t.as_str()),
        Some("cap.whistle")
    );
}

/// A renderer that records fills and texts, with a 6 × 10 cell font and
/// an optional touch screen.
struct Rec {
    caps: Caps,
    bottom: bool,
    on: u8,
    fills: Vec<(Rect, Color, u8)>,
    texts: Vec<(String, Vec2, f32, u8)>,
    sprites: Vec<(SpriteId, Vec2, SpriteParams)>,
    draws: u16,
}

impl Rec {
    fn new(caps: Caps, bottom: bool) -> Rec {
        Rec {
            caps,
            bottom,
            on: 0,
            fills: Vec::new(),
            texts: Vec::new(),
            sprites: Vec::new(),
            draws: 0,
        }
    }
}

impl Renderer for Rec {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: Color) {
        self.fills.clear();
        self.texts.clear();
        self.sprites.clear();
        self.on = 0;
    }
    fn draw_sprite(&mut self, id: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.sprites.push((id, pos, *params));
    }
    fn draw_model(
        &mut self,
        _: trino_core::ModelId,
        _: &trino_core::Transform3,
        _: trino_core::Material,
        _: &trino_core::ModelParams,
    ) {
    }
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fills.push((rect, color, self.on));
    }
    fn draw_text(&mut self, _: FontId, pos: Vec2, text: &str, params: &TextParams) {
        self.texts
            .push((text.to_string(), pos, params.scale, self.on));
    }
    fn measure_text(&self, _: FontId, text: &str, scale: f32) -> Vec2 {
        Vec2::new(text.chars().count() as f32 * 6.0 * scale, 10.0 * scale)
    }
    fn second_screen(&self) -> Option<Vec2> {
        self.bottom.then_some(Vec2::new(320.0, 240.0))
    }
    fn set_screen(&mut self, screen: u8) {
        if screen == 1 && self.bottom {
            self.on = 1;
        }
    }
    fn screen(&self) -> Screen {
        Screen::from_index(self.on).unwrap_or_default()
    }
    fn ui_stats(&mut self, s: trino_core::UiStats) {
        self.draws = s.draws;
    }
}

#[test]
fn captions_draw_a_readable_box_where_the_style_puts_it() {
    let data: &'static AccessData<'static> = Box::leak(Box::new(
        AccessData::parse(Box::leak(sample().into_boxed_slice())).unwrap(),
    ));
    let mut caps = Captions::new();
    caps.configure(Some(data), &settings(CaptionMode::All));
    caps.on_sound(asset_id("sounds/whistle"), None);
    // The N64: above the overscan margin, centered, a dark box.
    let mut r = Rec::new(Caps::N64, false);
    r.begin_frame(Color::BLACK);
    caps.draw(&mut r, &NoStrings, &CaptionStyle::DEFAULT);
    let (rect, color, on) = r.fills[0];
    assert_eq!(on, 0);
    assert_eq!(color, CaptionStyle::DEFAULT.background);
    assert!(rect.pos.y + rect.size.y <= 240.0 - 12.0, "{rect:?}");
    assert!((rect.pos.x + rect.size.x * 0.5 - 160.0).abs() <= 1.0);
    assert_eq!(r.texts[0].0, "cap.whistle");
    // At 1080p the text is 4 pixels a glyph pixel; at 200 % 8.
    let mut big = Rec::new(
        Caps {
            screen_width: 1920,
            screen_height: 1080,
            ..Caps::PC
        },
        false,
    );
    caps.draw(&mut big, &NoStrings, &CaptionStyle::DEFAULT);
    assert_eq!(big.texts[0].2, 4.0);
    caps.configure(
        Some(data),
        &AccessSettings {
            text_scale: 200,
            high_contrast: true,
            ..settings(CaptionMode::All)
        },
    );
    big.begin_frame(Color::BLACK);
    caps.draw(&mut big, &NoStrings, &CaptionStyle::DEFAULT);
    assert_eq!(big.texts[0].2, 8.0);
    // High contrast: opaque black, rules above and below.
    assert_eq!(big.fills[0].1, Color::BLACK);
    assert_eq!(big.fills.len(), 3);
    // The 3DS: on the touch screen when asked; one screen draws on its own.
    let style = CaptionStyle {
        place: CaptionPlace::SecondScreen,
        ..CaptionStyle::DEFAULT
    };
    let mut ds = Rec::new(Caps::N3DS, true);
    caps.draw(&mut ds, &NoStrings, &style);
    assert!(ds.fills.iter().all(|f| f.2 == 1), "{:?}", ds.fills);
    let mut one = Rec::new(Caps::N64, false);
    caps.draw(&mut one, &NoStrings, &style);
    assert!(one.fills.iter().all(|f| f.2 == 0));
    // Nothing queued, nothing drawn.
    caps.clear();
    let mut idle = Rec::new(Caps::N64, false);
    caps.draw(&mut idle, &NoStrings, &CaptionStyle::DEFAULT);
    assert!(idle.fills.is_empty() && idle.texts.is_empty());
}

// --------------------------------------------------- motion and flashes ----

#[test]
fn camera_effects_damp_under_reduced_motion_and_fade_out() {
    let calm = AccessSettings {
        reduce_motion: true,
        ..AccessSettings::DEFAULT
    };
    let full = AccessSettings::DEFAULT;
    let mut fx = CameraFx::new();
    fx.shake(0.5, 1.0);
    fx.punch(Vec3::new(0.0, 0.0, 1.0), 0.5);
    fx.kick_fov(0.2, 0.5);
    let mut biggest = (0.0f32, 0.0f32);
    for _ in 0..30 {
        fx.update(1.0 / 60.0);
        let (a, b) = (fx.offset(&full), fx.offset(&calm));
        biggest.0 = biggest.0.max(a.dot(a));
        biggest.1 = biggest.1.max(b.dot(b));
        assert_eq!(fx.fov_delta(&calm), 0.0, "no FOV kick when reduced");
    }
    assert!(biggest.0 > 0.0);
    assert!(biggest.1 <= biggest.0 * 0.2 * 0.2 + 1e-6, "{biggest:?}");
    let mut kick = CameraFx::new();
    kick.kick_fov(0.2, 0.5);
    kick.shake(0.5, 1.0);
    kick.update(0.1);
    let mut cam = trino_core::render3d::Camera3::default();
    let before = cam;
    kick.apply(&mut cam, &full);
    assert!(cam.fov_y > before.fov_y);
    // Eye and target move together (a shake, not a turn).
    let (de, dt) = (cam.eye - before.eye, cam.target - before.target);
    let d = de - dt;
    assert!(d.dot(d) < 1e-10, "{de:?} vs {dt:?}");
    for _ in 0..120 {
        fx.update(1.0 / 60.0);
    }
    assert!(!fx.active());
    assert_eq!(fx.offset(&full), Vec3::ZERO);
}

#[test]
fn the_flash_guard_holds_a_synthetic_strobe_to_three_a_second() {
    let guard = AccessSettings::DEFAULT;
    assert!(guard.flash_guard, "on by default");
    let open = AccessSettings {
        flash_guard: false,
        ..guard
    };
    // A 12 Hz strobe for two seconds at 60 frames a second.
    for (s, want) in [(guard, 2 * MAX_FLASHES_PER_SECOND), (open, 24)] {
        let mut flash = ScreenFlash::new();
        let mut monitor = FlashMonitor::new();
        let mut shown = 0;
        let mut r = Rec::new(Caps::N64, false);
        for f in 0..120 {
            if f % 5 == 0 {
                shown += flash.trigger(Color::WHITE, 1.0 / 30.0, &s) as usize;
            }
            r.begin_frame(Color::BLACK);
            flash.draw(&mut r);
            let level = r
                .fills
                .iter()
                .map(|(rect, c, _)| {
                    FlashMonitor::level_of(*c, rect.size.x * rect.size.y / (320.0 * 240.0))
                })
                .fold(0.0, f32::max);
            monitor.frame(f as f32 / 60.0, level);
            flash.update(1.0 / 60.0);
        }
        assert_eq!(shown, want, "flash_guard {}", s.flash_guard);
        if s.flash_guard {
            assert_eq!(monitor.strobes, 0, "the guarded flashes never strobe");
            assert!(flash.current().a <= trino_core::access::motion::GUARDED_ALPHA);
        } else {
            assert_eq!(monitor.strobes, 1, "the raw strobe is found once");
        }
    }
}

#[test]
fn hold_to_toggle_latches_a_held_action() {
    let hold = AccessSettings::DEFAULT;
    let toggle = AccessSettings {
        hold_to_toggle: true,
        ..hold
    };
    let mut down = InputState::default();
    down.set(Button::B, true);
    let up = InputState::default();
    let run = |s: &AccessSettings| {
        let mut h = HoldToggle::new();
        [
            h.update(&down, &up, Button::B, s),
            h.update(&down, &down, Button::B, s),
            h.update(&up, &down, Button::B, s),
            h.update(&down, &up, Button::B, s),
            h.update(&up, &down, Button::B, s),
        ]
    };
    assert_eq!(run(&hold), [true, true, false, true, false]);
    assert_eq!(run(&toggle), [true, true, true, false, false]);
}

// ------------------------------------------------------------------- UI ----

fn screen(nodes: &'static [UiNode<'static>]) -> UiScreen<'static> {
    let s = UiScreen {
        name: "access",
        reference: (320, 240),
        scaling: Scaling::Integer,
        wrap: false,
        focus: NONE,
        on_back: "",
        theme: Theme::DEFAULT,
        nodes: Nodes::Slice(nodes),
    };
    s.check_tree().unwrap();
    s
}

static NODES: [UiNode<'static>; 3] = [
    UiNode {
        size: (Size::Fill, Size::Fill),
        end: 3,
        ..UiNode::DEFAULT
    },
    UiNode {
        kind: NodeKind::Button,
        id: "go",
        text: "GO",
        action: "go",
        parent: 0,
        end: 2,
        anchor: Anchor::Center,
        size: (Size::Px(80), Size::Px(24)),
        color: Some(Color::rgb(200, 0, 0)),
        cvd_icon: Some(CvdIcon {
            sprite: SpriteId::from_path("sprites/stripes"),
            size: (8, 8),
        }),
        ..UiNode::DEFAULT
    },
    UiNode {
        kind: NodeKind::Text,
        text: "HELLO",
        parent: 0,
        end: 3,
        anchor: Anchor::TopLeft,
        ..UiNode::DEFAULT
    },
];

fn drawn(caps: Caps, access: UiAccess) -> Rec {
    let s = screen(&NODES);
    let mut r = Rec::new(caps, false);
    let mut ui: Ui<8> = Ui::new();
    let env = UiEnv {
        access,
        ..UiEnv::of(&r)
    };
    r.begin_frame(Color::BLACK);
    ui.draw(&mut r, &s, &env);
    r
}

#[test]
fn the_ui_scales_text_in_whole_pixels_and_keeps_the_default_draws() {
    let n64 = drawn(Caps::N64, UiAccess::NONE);
    // UiAccess::NONE and `with_access(DEFAULT)` draw what a screen drew
    // before the settings existed.
    let s = screen(&NODES);
    let mut same = Rec::new(Caps::N64, false);
    let mut ui: Ui<8> = Ui::new();
    let env = UiEnv::of(&same).with_access(&AccessSettings::DEFAULT);
    ui.draw(&mut same, &s, &env);
    assert_eq!(same.fills, n64.fills);
    assert_eq!(same.texts, n64.texts);
    assert_eq!(n64.texts[0].2, 1.0);
    let at = |t: f32| UiAccess {
        text_scale: t,
        ..UiAccess::NONE
    };
    // The consoles (×1): whole glyph pixels, 150 % already draws at 2.
    assert_eq!(drawn(Caps::N64, at(1.25)).texts[0].2, 1.0);
    assert_eq!(drawn(Caps::N64, at(1.5)).texts[0].2, 2.0);
    assert_eq!(drawn(Caps::N64, at(2.0)).texts[0].2, 2.0);
    // 1080p (×4): 150 % is 6 pixels a glyph pixel.
    let hd = Caps {
        screen_width: 1920,
        screen_height: 1080,
        ..Caps::PC
    };
    assert_eq!(drawn(hd, UiAccess::NONE).texts[0].2, 4.0);
    assert_eq!(drawn(hd, at(1.5)).texts[0].2, 6.0);
    // A Fit text grows with it: its box is laid out at the new size.
    let small = drawn(Caps::N64, UiAccess::NONE);
    let big = drawn(Caps::N64, at(2.0));
    let hello = |r: &Rec| r.texts.iter().find(|t| t.0 == "HELLO").unwrap().1;
    assert_eq!(hello(&small), hello(&big), "anchored top-left either way");
}

#[test]
fn high_contrast_and_color_vision_icons_change_only_the_look() {
    let plain = drawn(Caps::N64, UiAccess::NONE);
    // The button's own red, no icon.
    assert!(plain.fills.iter().any(|f| f.1 == Color::rgb(200, 0, 0)));
    assert!(plain.sprites.is_empty());
    let hc = drawn(
        Caps::N64,
        UiAccess {
            high_contrast: true,
            ..UiAccess::NONE
        },
    );
    // High contrast: the theme's black button with a white outline, never
    // the node's red.
    assert!(!hc.fills.iter().any(|f| f.1 == Color::rgb(200, 0, 0)));
    assert!(hc.fills.iter().any(|f| f.1 == Theme::HIGH_CONTRAST.button));
    assert!(hc.fills.iter().any(|f| f.1 == Theme::HIGH_CONTRAST.text));
    // A project theme replaces the engine's colors.
    let own = drawn(
        Caps::N64,
        UiAccess {
            high_contrast: true,
            contrast: Some(Theme {
                button: Color::rgb(0, 0, 90),
                ..Theme::HIGH_CONTRAST
            }),
            ..UiAccess::NONE
        },
    );
    assert!(own.fills.iter().any(|f| f.1 == Color::rgb(0, 0, 90)));
    // A color-vision mode adds the icon at the button's right end.
    let icons = drawn(
        Caps::N64,
        UiAccess {
            cvd: CvdMode::Deuteranopia,
            ..UiAccess::NONE
        },
    );
    assert_eq!(icons.sprites.len(), 1);
    let (id, pos, params) = icons.sprites[0];
    assert_eq!(id, SpriteId::from_path("sprites/stripes"));
    assert_eq!(params.scale, Vec2::new(1.0, 1.0));
    assert_eq!(pos, Vec2::new(120.0 + 80.0 - 8.0, 108.0 + 8.0));
    assert_eq!(icons.draws, plain.draws + 1);
}

#[test]
fn a_settings_change_lays_the_screen_out_again() {
    let s = screen(&NODES);
    let mut r = Rec::new(Caps::N64, false);
    let mut ui: Ui<8> = Ui::new();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    let text_rect = |ui: &Ui<8>| ui.rect(2);
    let before = text_rect(&ui);
    ui.draw(
        &mut r,
        &s,
        &env.with_access(&AccessSettings {
            text_scale: 200,
            ..AccessSettings::DEFAULT
        }),
    );
    let after = text_rect(&ui);
    assert_eq!(after.size.x, before.size.x * 2.0);
    assert_eq!(after.size.y, before.size.y * 2.0);
}

#[test]
fn settings_pack_into_three_bytes_for_a_console_save() {
    let s = AccessSettings {
        cvd: CvdMode::Tritanopia,
        text_scale: 150,
        high_contrast: true,
        captions: CaptionMode::All,
        reduce_motion: true,
        flash_guard: false,
        hold_to_toggle: true,
    };
    let b = s.pack();
    assert_eq!(b.len(), trino_core::access::PACKED_LEN);
    assert_eq!(AccessSettings::unpack(b), Some(s));
    // Out-of-range text scales clamp before packing.
    let wild = AccessSettings {
        text_scale: 250,
        ..AccessSettings::DEFAULT
    };
    assert_eq!(AccessSettings::unpack(wild.pack()).unwrap().text_scale, 200);
}

// ------------------------------------------------------------ TUIL v2 ----

/// TUIL version 2 carries a node's color-vision icon; a version 1 blob
/// (94-byte records, no icon) still loads (issue #60's format change).
#[test]
fn tuil_v2_carries_cvd_icons_and_v1_still_loads() {
    use trino_core::ui::tuil;
    let s = screen(&NODES);
    let mut pool = String::new();
    let mut refs = std::collections::HashMap::new();
    for i in 0..s.len() {
        let n = s.node(i);
        for t in [
            n.id,
            n.text,
            n.key,
            n.bind,
            n.action,
            n.visible_if,
            n.enabled_if,
        ] {
            if !refs.contains_key(t) {
                refs.insert(t.to_string(), (pool.len() as u16, t.len() as u16));
                pool.push_str(t);
            }
        }
    }
    for t in [s.name, s.on_back] {
        if !refs.contains_key(t) {
            refs.insert(t.to_string(), (pool.len() as u16, t.len() as u16));
            pool.push_str(t);
        }
    }
    let mut find = |t: &str| refs[t];
    let mut v2 = tuil::encode_header(&s, s.len(), pool.len(), &mut find).to_vec();
    let mut records = Vec::new();
    for i in 0..s.len() {
        records.push(tuil::encode_record(&s.node(i), &mut find));
    }
    for r in &records {
        v2.extend_from_slice(r);
    }
    v2.extend_from_slice(pool.as_bytes());
    let parsed = UiScreen::parse(&v2).unwrap();
    assert_eq!(parsed.node(1).cvd_icon, NODES[1].cvd_icon);
    assert_eq!(parsed.node(2).cvd_icon, None);
    // The same screen as version 1 wrote it: no icons, every other field.
    let mut v1 = v2[..tuil::HEADER].to_vec();
    v1[4..6].copy_from_slice(&1u16.to_le_bytes());
    v1[14..16].copy_from_slice(&(tuil::RECORD_V1 as u16).to_le_bytes());
    let pool_at = (tuil::HEADER + s.len() * tuil::RECORD_V1) as u32;
    v1[32..36].copy_from_slice(&pool_at.to_le_bytes());
    for r in &records {
        v1.extend_from_slice(&r[..tuil::RECORD_V1]);
    }
    v1.extend_from_slice(pool.as_bytes());
    let old = UiScreen::parse(&v1).unwrap();
    assert_eq!(old.node(1).cvd_icon, None);
    assert_eq!(old.node(1).text, "GO");
    assert_eq!(old.node(1).color, NODES[1].color);
    // A version 1 header with version 2's record size is fine; the other
    // way round is short.
    let mut short = v2.clone();
    short[14..16].copy_from_slice(&(tuil::RECORD_V1 as u16).to_le_bytes());
    assert!(UiScreen::parse(&short).is_err());
}

// ------------------------------------------------------------ outlines ----

/// Screen bounds of a run of triangles.
fn bounds(tris: &[trino_core::ScreenTri]) -> (Vec2, Vec2) {
    let mut lo = Vec2::new(f32::MAX, f32::MAX);
    let mut hi = Vec2::new(f32::MIN, f32::MIN);
    for p in tris.iter().flat_map(|t| t.pts) {
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

/// The nearest normalized depth of `tris` at screen point `p` (`1/z`, and
/// so `1 - NEAR/z`, is affine on screen).
fn depth_at(tris: &[trino_core::ScreenTri], p: Vec2) -> Option<f32> {
    let mut best: Option<f32> = None;
    for t in tris {
        let [a, b, c] = t.pts;
        let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if area.abs() < 1e-6 {
            continue;
        }
        let w1 = ((p.x - a.x) * (c.y - a.y) - (p.y - a.y) * (c.x - a.x)) / area;
        let w2 = ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)) / area;
        let w0 = 1.0 - w1 - w2;
        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
            continue;
        }
        let z = w0 * t.z[0] + w1 * t.z[1] + w2 * t.z[2];
        best = Some(best.map_or(z, |b| b.min(z)));
    }
    best
}

#[test]
fn an_outline_is_a_larger_unlit_copy_behind_the_object() {
    use common::{Call, Recorder, bounded_cube};
    use trino_core::access::{OUTLINE_COLOR, draw_outlined};
    use trino_core::{Camera3, Material, ModelId, ModelParams, Outline, Shading, Transform3};

    let cube = ModelId::from_path("models/cube");
    let camera = Camera3 {
        eye: Vec3::new(0.0, 1.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let at = Transform3::default();
    let params = ModelParams::DEFAULT;
    let ring = Outline::new(OUTLINE_COLOR, Vec3::ZERO, 0.87).with_pixels(3.0);

    // Without an outline: exactly the plain draw.
    let mut plain = Recorder::new();
    plain.add_mesh(cube, bounded_cube());
    plain.set_camera(&camera);
    plain.draw_model(cube, &at, Material::VertexLit, &params);
    let mut off = Recorder::new();
    off.add_mesh(cube, bounded_cube());
    off.set_camera(&camera);
    draw_outlined(
        &mut off,
        &camera,
        cube,
        &at,
        Material::VertexLit,
        &params,
        None,
    );
    assert_eq!(off.calls, plain.calls);

    // With one: the ring's copy first, then the object, unchanged.
    let mut on = Recorder::new();
    on.add_mesh(cube, bounded_cube());
    on.set_camera(&camera);
    draw_outlined(
        &mut on,
        &camera,
        cube,
        &at,
        Material::VertexLit,
        &params,
        Some(&ring),
    );
    assert_eq!(on.calls.len(), 2);
    assert_eq!(on.calls[1], plain.calls[0]);
    let Call::Model {
        params: copy,
        emitted,
        ..
    } = &on.calls[0]
    else {
        panic!("the ring is a model draw: {:?}", on.calls[0]);
    };
    assert_eq!(copy.shading, Shading::Unlit);
    assert_eq!(copy.emissive, OUTLINE_COLOR);
    let (ring_tris, object_tris) = on.tris.split_at(*emitted);
    assert!(ring_tris.iter().all(|t| t.colors == [OUTLINE_COLOR; 3]));

    // Behind per pixel (what the z-buffer compares): wherever the object
    // covers the screen the copy is deeper, its slanted top face too, so the
    // object draws whole.
    let (lo, hi) = bounds(object_tris);
    let mut checked = 0;
    for y in lo.y as i32..=hi.y as i32 {
        for x in lo.x as i32..=hi.x as i32 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            if let (Some(o), Some(c)) = (depth_at(object_tris, p), depth_at(ring_tris, p)) {
                assert!(c > o, "the copy covers the object at {p:?}");
                checked += 1;
            }
        }
    }
    assert!(checked > 1000);

    // Larger on every side, by about the ring's pixels: exactly that at the
    // sphere's center depth, less where the silhouette is nearer the camera
    // (the cube's front edges), more where it is farther.
    let (rlo, rhi) = bounds(ring_tris);
    for grow in [lo.x - rlo.x, lo.y - rlo.y, rhi.x - hi.x, rhi.y - hi.y] {
        assert!((1.0..=6.0).contains(&grow), "grew {grow} px");
    }
}
