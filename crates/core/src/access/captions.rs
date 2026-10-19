//! Captions for sounds: the manifest gives a sound `caption = "key"`
//! (`caption_priority`, `important`, `direction`), the bake puts it in
//! `access/data`, and [`Captions`] shows it while the player's
//! [`CaptionMode`] wants it.
//!
//! ```ignore
//! // update_with:
//! let settings = platform.access();
//! self.captions.configure(self.access.get(platform), &settings);
//! let mut audio = self.captions.wrap(audio); // plays through, captions what it plays
//! self.update(input, &mut audio, dt);
//! self.captions.update(dt);
//! // render, last (the 3DS's touch-screen placement switches screens):
//! self.captions.draw(renderer, strings, &CaptionStyle::DEFAULT);
//! ```
//!
//! Captions are 2D text over a box: fills and glyphs every target has.

use core::fmt::Write as _;

use super::data::{AccessData, CaptionInfo};
use super::{AccessSettings, CaptionMode};
use crate::audio::{Audio, AudioStats, Bus, DuckRule, MusicId, SoundId, SoundParams, VoiceId};
use crate::math::{Color, Rect, Vec2};
use crate::render::{FontId, Renderer, Screen, TextAlign, TextParams};
use crate::text::TextBuf;
use crate::ui::UiStrings;

/// Captions shown at once.
pub const MAX_CAPTIONS: usize = 4;
/// How long a caption stays when its sound gives no duration.
pub const DEFAULT_SECONDS: f32 = 3.0;
/// How far a sound must be panned for its caption to point at it.
pub const DIRECTION_PAN: f32 = 0.25;

/// Where the captions go.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CaptionPlace {
    /// Above the bottom edge of the main screen (subtitles' place).
    #[default]
    Bottom,
    /// Under the top edge of the main screen.
    Top,
    /// The 3DS touch screen; the main screen's bottom where there is none.
    /// Draw the captions last: the frame moves to the second screen.
    SecondScreen,
}

/// How captions look.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CaptionStyle {
    pub font: FontId,
    pub place: CaptionPlace,
    /// The box behind each line (readable over any scene).
    pub background: Color,
    pub text: Color,
}

impl CaptionStyle {
    /// The engine's 5×7 font, white on a near-opaque black box, at the bottom.
    pub const DEFAULT: CaptionStyle = CaptionStyle {
        font: FontId::from_path("fonts/trino5x7"),
        place: CaptionPlace::Bottom,
        background: Color::rgba(0, 0, 0, 210),
        text: Color::WHITE,
    };
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: &'static str,
    priority: u8,
    left: f32,
    /// The sound's pan when the caption points at it.
    pan: Option<f32>,
    count: u8,
    seq: u32,
}

/// The caption queue.
#[derive(Clone, Copy, Debug)]
pub struct Captions {
    data: AccessData<'static>,
    mode: CaptionMode,
    text_factor: f32,
    high_contrast: bool,
    entries: [Option<Entry>; MAX_CAPTIONS],
    seq: u32,
}

impl Captions {
    pub const fn new() -> Captions {
        Captions {
            data: AccessData::EMPTY,
            mode: CaptionMode::Off,
            text_factor: 1.0,
            high_contrast: false,
            entries: [None; MAX_CAPTIONS],
            seq: 0,
        }
    }

    /// The project's caption table and the player's settings (each frame:
    /// both can change at run time).
    pub fn configure(&mut self, data: Option<&AccessData<'static>>, settings: &AccessSettings) {
        self.data = data.copied().unwrap_or(AccessData::EMPTY);
        if settings.captions != self.mode && settings.captions == CaptionMode::Off {
            self.clear();
        }
        self.mode = settings.captions;
        self.text_factor = settings.text_factor();
        self.high_contrast = settings.high_contrast;
    }

    pub fn mode(&self) -> CaptionMode {
        self.mode
    }

    /// A sound (or music track) just started: queue its caption when it has
    /// one the mode shows. `pan` (-1 left .. 1 right) points the caption at
    /// it when the sound wants a direction. `true` when queued.
    pub fn on_sound(&mut self, id: u32, pan: Option<f32>) -> bool {
        match self.data.caption(id) {
            Some(info) => self.push(info, pan),
            None => false,
        }
    }

    /// Queue a caption the game makes itself (a voice line, an event).
    pub fn push(&mut self, info: CaptionInfo<'static>, pan: Option<f32>) -> bool {
        if !self.mode.shows(info.important) || info.key.is_empty() {
            return false;
        }
        let left = if info.duration_ms > 0 {
            info.duration_ms as f32 / 1000.0
        } else {
            DEFAULT_SECONDS
        };
        let pan = pan.filter(|p| info.direction && p.abs() >= DIRECTION_PAN);
        self.seq = self.seq.wrapping_add(1);
        // The same caption again: it stays longer and counts.
        if let Some(e) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|e| e.key == info.key)
        {
            e.left = e.left.max(left);
            e.count = e.count.saturating_add(1);
            e.pan = pan;
            e.seq = self.seq;
            return true;
        }
        let entry = Entry {
            key: info.key,
            priority: info.priority,
            left,
            pan,
            count: 1,
            seq: self.seq,
        };
        if let Some(free) = self.entries.iter_mut().find(|e| e.is_none()) {
            *free = Some(entry);
            return true;
        }
        // Full: the lowest priority goes (the oldest of those), unless the
        // new one is lower still.
        let (slot, worst) = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.map(|e| (i, e)))
            .min_by_key(|(_, e)| (e.priority, e.seq))
            .expect("the queue is full");
        if worst.priority > info.priority {
            return false;
        }
        self.entries[slot] = Some(entry);
        true
    }

    pub fn update(&mut self, dt: f32) {
        for slot in self.entries.iter_mut() {
            if let Some(e) = slot {
                e.left -= dt;
                if e.left <= 0.0 {
                    *slot = None;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries = [None; MAX_CAPTIONS];
    }

    /// Captions on screen now.
    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The captions' keys, oldest first.
    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.ordered().map(|e| e.key)
    }

    /// The lines [`draw`](Self::draw) shows, oldest first: the text in the
    /// player's language, the arrow, the repeat count (a game that draws
    /// captions its own way).
    pub fn texts(&self, strings: &dyn UiStrings) -> [Option<TextBuf<96>>; MAX_CAPTIONS] {
        let mut out = [None; MAX_CAPTIONS];
        for (slot, e) in out.iter_mut().zip(self.ordered()) {
            let mut buf = TextBuf::<96>::new();
            line(&mut buf, e, strings);
            *slot = Some(buf);
        }
        out
    }

    fn ordered(&self) -> impl Iterator<Item = Entry> + '_ {
        let mut order = [usize::MAX; MAX_CAPTIONS];
        let mut n = 0;
        for (i, e) in self.entries.iter().enumerate() {
            if e.is_some() {
                order[n] = i;
                n += 1;
            }
        }
        order[..n].sort_unstable_by_key(|&i| self.entries[i].map_or(0, |e| e.seq));
        (0..n).filter_map(move |k| self.entries[order[k]])
    }

    /// The pixel scale captions use on a `screen_h`-tall screen: the UI's
    /// whole-pixel scale (1 at 240 lines, 4 at 1080p) times the text scale,
    /// rounded to whole pixels.
    pub fn text_px(&self, screen_h: f32) -> f32 {
        let base = ((screen_h / 240.0) as i32).max(1) as f32;
        let px = base * self.text_factor;
        (px + 0.5) as i32 as f32
    }

    /// Where each caption's box goes on a `size` screen with `safe` insets
    /// (pixels), oldest first.
    pub fn boxes(
        &self,
        renderer: &dyn Renderer,
        strings: &dyn UiStrings,
        style: &CaptionStyle,
        size: Vec2,
        place: CaptionPlace,
    ) -> [Option<Rect>; MAX_CAPTIONS] {
        let px = self.text_px(size.y);
        let pad = 2.0 * px;
        let line_h = renderer.measure_text(style.font, "", px).y.max(px * 7.0);
        let safe = renderer.safe_area(renderer.screen());
        let gap = px;
        let n = self.len();
        let mut out = [None; MAX_CAPTIONS];
        for (k, e) in self.ordered().enumerate() {
            let mut buf = TextBuf::<96>::new();
            let text = line(&mut buf, e, strings);
            let w = renderer.measure_text(style.font, text, px).x + 2.0 * pad;
            let h = line_h + 2.0 * pad;
            let x = ((size.x - w) * 0.5).max(0.0);
            let y = match place {
                CaptionPlace::Top => safe.top as f32 + 4.0 * px + k as f32 * (h + gap),
                _ => {
                    // Newest at the bottom.
                    let from_bottom = (n - 1 - k) as f32;
                    size.y - safe.bottom as f32 - 4.0 * px - h - from_bottom * (h + gap)
                }
            };
            out[k] = Some(Rect::new(x, y, w, h));
        }
        out
    }

    /// Draw the queue: a box per caption with its text in the player's
    /// language (`strings`; the key itself where no table has it), an arrow
    /// toward a panned sound, `(x2)` when it repeated.
    pub fn draw(&self, renderer: &mut dyn Renderer, strings: &dyn UiStrings, style: &CaptionStyle) {
        if self.is_empty() {
            return;
        }
        let mut place = style.place;
        if place == CaptionPlace::SecondScreen && !renderer.on_screen(Screen::Bottom) {
            place = CaptionPlace::Bottom;
        }
        let Some(size) = renderer.screen_size(renderer.screen()) else {
            return;
        };
        let px = self.text_px(size.y);
        let pad = 2.0 * px;
        let (bg, fg) = if self.high_contrast {
            (Color::BLACK, Color::WHITE)
        } else {
            (style.background, style.text)
        };
        let boxes = self.boxes(renderer, strings, style, size, place);
        for (e, r) in self.ordered().zip(boxes.iter()) {
            let Some(r) = *r else { continue };
            renderer.fill_rect(r, bg);
            if self.high_contrast {
                let w = px.max(1.0);
                let accent = Color::rgb(255, 230, 0);
                renderer.fill_rect(Rect::new(r.pos.x, r.pos.y, r.size.x, w), accent);
                renderer.fill_rect(
                    Rect::new(r.pos.x, r.pos.y + r.size.y - w, r.size.x, w),
                    accent,
                );
            }
            let mut buf = TextBuf::<96>::new();
            let text = line(&mut buf, e, strings);
            renderer.draw_text(
                style.font,
                Vec2::new(r.pos.x + pad, r.pos.y + pad),
                text,
                &TextParams {
                    scale: px,
                    tint: fg,
                    align: TextAlign::Left,
                },
            );
        }
    }

    /// Play through `audio` and queue the caption of every sound, track and
    /// stinger started through the wrapper.
    pub fn wrap<'a>(&'a mut self, audio: &'a mut dyn Audio) -> Captioned<'a> {
        Captioned {
            inner: audio,
            captions: self,
        }
    }
}

impl Default for Captions {
    fn default() -> Self {
        Self::new()
    }
}

fn line<'b>(buf: &'b mut TextBuf<96>, e: Entry, strings: &dyn UiStrings) -> &'b str {
    buf.clear();
    let text = strings.get(e.key).unwrap_or(e.key);
    if e.pan.is_some_and(|p| p < 0.0) {
        let _ = buf.write_str("< ");
    }
    let _ = buf.write_str(text);
    if e.count > 1 {
        let _ = write!(buf, " (x{})", e.count);
    }
    if e.pan.is_some_and(|p| p > 0.0) {
        let _ = buf.write_str(" >");
    }
    buf.as_str()
}

/// [`Audio`] that plays through the game's backend and captions what it
/// plays ([`Captions::wrap`]). Every method forwards.
pub struct Captioned<'a> {
    inner: &'a mut dyn Audio,
    captions: &'a mut Captions,
}

impl Captioned<'_> {
    /// The queue (to push the game's own captions mid-frame).
    pub fn captions(&mut self) -> &mut Captions {
        self.captions
    }
}

impl Audio for Captioned<'_> {
    fn play_sound(&mut self, sound: SoundId) {
        self.inner.play_sound(sound);
        self.captions.on_sound(sound.0, None);
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        self.inner.play_music(music, looped);
        self.captions.on_sound(music.0, None);
    }

    fn stop_music(&mut self) {
        self.inner.stop_music();
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.inner.set_master_volume(volume);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        self.inner.play_sound_with(sound, params);
        self.captions.on_sound(sound.0, Some(params.pan));
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let v = self.inner.play_voice(sound, params, looped);
        if v.is_some() {
            self.captions.on_sound(sound.0, Some(params.pan));
        }
        v
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        self.inner.set_voice(voice, params);
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        self.inner.stop_voice(voice);
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.inner.voice_playing(voice)
    }

    fn play_stinger(&mut self, music: MusicId) {
        self.inner.play_stinger(music);
        self.captions.on_sound(music.0, None);
    }

    fn stinger_playing(&self) -> bool {
        self.inner.stinger_playing()
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.inner.set_music_volume(volume);
    }

    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.inner.set_bus_volume(bus, volume);
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        self.inner.bus_volume(bus)
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.inner.set_bus_muted(bus, muted);
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        self.inner.bus_muted(bus)
    }

    fn set_duck(&mut self, rule: DuckRule) {
        self.inner.set_duck(rule);
    }

    fn clear_ducks(&mut self) {
        self.inner.clear_ducks();
    }

    fn bus_duck(&self, bus: Bus) -> f32 {
        self.inner.bus_duck(bus)
    }

    fn audio_stats(&self) -> AudioStats {
        self.inner.audio_stats()
    }
}
