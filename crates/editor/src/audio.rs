//! Audio in the editor (issue #53): what each platform bakes a sound or a
//! track to, measured like the bake (`asset_pipeline::audio::encode`, the
//! same call `cargo xtask audio inspect` makes) on a worker thread; the
//! project's audio memory per platform for the budgets; a preview player
//! (the PC backend) that plays the master or any platform's decoded
//! result; and the project mix (`assets/audio.toml`) the Mixer tab edits.
//! Measurements are keyed on the manifest's text and the project
//! generation (like the texture inspector), so an edit from the
//! inspector, an agent's edit or a master change recomputes them.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use trino_asset_pipeline::audio::{
    self, AudioSettings, Codec, Encoded, Footprint, Master, MixFile,
};
use trino_asset_pipeline::{Manifest, Platform};
use trino_core::{Audio, Bus, MixerConfig, SoundId, SoundParams, VoiceId};

pub use crate::textures::{Key, key};

/// A codec as people write it.
pub fn codec_name(c: Codec) -> &'static str {
    match c {
        Codec::Pcm => "PCM16",
        Codec::Vorbis => "Vorbis",
        Codec::Vadpcm => "VADPCM",
        Codec::Opus => "Opus",
        Codec::DspAdpcm => "DSP-ADPCM",
    }
}

/// The SNR a lossy codec should keep on real content (what the codec
/// tests assert on their fixtures, `asset-pipeline/tests/audio_codecs.rs`):
/// the inspector colors a result under it amber. PCM has none, and the
/// N64's Opus doesn't decode on the host.
pub fn snr_floor(c: Codec) -> Option<f64> {
    match c {
        Codec::Vadpcm => Some(25.0),
        Codec::DspAdpcm => Some(30.0),
        Codec::Vorbis => Some(26.0),
        Codec::Opus | Codec::Pcm => None,
    }
}

/// What a platform calls the place its audio is stored.
pub fn storage_name(p: Platform) -> &'static str {
    match p {
        Platform::Pc => "disk",
        Platform::N64 => "ROM",
        Platform::N3ds => "RomFS",
        Platform::Web => "download",
    }
}

pub fn platform_label(p: Platform) -> &'static str {
    match p {
        Platform::Pc => "PC",
        Platform::N64 => "N64",
        Platform::N3ds => "3DS",
        Platform::Web => "Web",
    }
}

/// Buckets of a waveform's envelope.
pub const WAVE_BUCKETS: usize = 480;

/// A waveform's min/max per bucket (the inspector paints it at any width).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wave {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}

impl Wave {
    pub fn of(samples: &[f32]) -> Wave {
        let n = samples.len();
        let buckets = WAVE_BUCKETS.min(n.max(1));
        let mut w = Wave {
            min: vec![0.0; buckets],
            max: vec![0.0; buckets],
        };
        for b in 0..buckets {
            let (lo, hi) = (
                b * n / buckets,
                ((b + 1) * n / buckets).max(b * n / buckets + 1),
            );
            let s = &samples[lo.min(n)..hi.min(n)];
            w.min[b] = s.iter().copied().fold(0.0, f32::min);
            w.max[b] = s.iter().copied().fold(0.0, f32::max);
        }
        w
    }
}

/// One platform's column of the inspector.
pub struct Column {
    pub platform: Platform,
    /// The manifest's `codec` / `sample_rate` for this platform (`None`:
    /// the default, `auto` / the master's rate).
    pub declared_codec: Option<String>,
    pub declared_rate: Option<u32>,
    /// The bake's result, or why it fails.
    pub result: Result<Measured, String>,
}

/// A platform's bake of the sound, measured.
pub struct Measured {
    pub settings: AudioSettings,
    pub rate: u32,
    pub frames: usize,
    pub bytes: usize,
    /// `false`: N64 Opus, whose size is an estimate (`audioconv64` makes it).
    pub exact: bool,
    pub pcm16_bytes: usize,
    /// Against the (resampled) master; `None`: not decodable on the host.
    pub snr: Option<f64>,
    pub footprint: Footprint,
    /// What the platform plays (`None`: N64 Opus).
    pub decoded: Option<Vec<i16>>,
    pub wave: Option<Wave>,
    /// |decoded − master|, the codec's noise.
    pub noise: Option<Wave>,
}

impl Measured {
    /// Smaller than PCM16 of the same samples by this factor.
    pub fn ratio(&self) -> f64 {
        self.pcm16_bytes as f64 / self.bytes.max(1) as f64
    }
}

/// A sound or track as every platform bakes it.
pub struct Inspection {
    pub logical: String,
    pub music: bool,
    pub file: String,
    /// The PC master (what the waveform shows).
    pub master: Result<(u32, Vec<i16>), String>,
    pub master_wave: Wave,
    /// The manifest's own keys (`None`: the default).
    pub quality: Option<f32>,
    pub stream: Option<bool>,
    pub looped: Option<bool>,
    pub bus: Option<String>,
    pub columns: Vec<Column>,
}

/// A sound's caption keys as the manifest has them (`None`: unset), read
/// apart from the codec measurements (a checkbox must not re-measure
/// every platform).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionKeys {
    pub key: Option<String>,
    pub important: Option<bool>,
    pub priority: Option<u8>,
    pub direction: Option<bool>,
    pub seconds: Option<f32>,
}

impl Inspection {
    /// The settings the PC bakes with (the defaults the fields show).
    pub fn settings(&self) -> Option<&AudioSettings> {
        self.columns
            .iter()
            .find_map(|c| c.result.as_ref().ok().map(|m| &m.settings))
    }
}

/// `sounds/x` or `music/x`: (music?, name).
pub fn split(logical: &str) -> Result<(bool, &str), String> {
    match logical.split_once('/') {
        Some(("sounds", n)) => Ok((false, n)),
        Some(("music", n)) => Ok((true, n)),
        _ => Err(format!("`{logical}` is not a sound or music track")),
    }
}

/// Measure `logical` on every platform, like the bake.
pub fn measure(assets: &Path, logical: &str) -> Inspection {
    let mut insp = Inspection {
        logical: logical.to_string(),
        music: logical.starts_with("music/"),
        file: String::new(),
        master: Err("not measured".into()),
        master_wave: Wave::default(),
        quality: None,
        stream: None,
        looped: None,
        bus: None,
        columns: Vec::new(),
    };
    let fail = |insp: &mut Inspection, e: String| {
        insp.master = Err(e.clone());
        insp.columns = Platform::ALL
            .into_iter()
            .map(|platform| Column {
                platform,
                declared_codec: None,
                declared_rate: None,
                result: Err(e.clone()),
            })
            .collect();
    };
    let manifest = match Manifest::load(&assets.join("manifest.toml")) {
        Ok(m) => m,
        Err(e) => {
            fail(&mut insp, e);
            return insp;
        }
    };
    let (music, name) = match split(logical) {
        Ok(v) => v,
        Err(e) => {
            fail(&mut insp, e);
            return insp;
        }
    };
    let table = if music {
        &manifest.music
    } else {
        &manifest.sounds
    };
    let Some(decl) = table.get(name) else {
        fail(&mut insp, format!("`{logical}` is not in the manifest"));
        return insp;
    };
    insp.file = decl.file.clone();
    insp.quality = decl.quality;
    insp.stream = decl.stream;
    insp.looped = decl.looped;
    insp.bus = decl.bus.clone();
    let id = trino_core::asset_id(logical);
    // A platform override (`assets/n64/...`) has its own master.
    let mut masters: Vec<(PathBuf, Result<Master, String>)> = Vec::new();
    for platform in Platform::ALL {
        let declared_codec = decl.codec.as_ref().and_then(|c| c.get(platform));
        let declared_rate = decl.sample_rate.as_ref().and_then(|r| r.get(platform));
        let result = (|| -> Result<Measured, String> {
            let settings = decl.audio_settings(music, platform)?;
            let source = trino_asset_pipeline::resolve_source(assets, platform, &decl.file)?;
            let at = match masters.iter().position(|(p, _)| *p == source) {
                Some(i) => i,
                None => {
                    let m = trino_asset_pipeline::decode_master(&source);
                    masters.push((source, m));
                    masters.len() - 1
                }
            };
            let master = masters[at].1.clone()?;
            let enc = audio::encode(platform, &settings, &master, id)?;
            let snr = enc.snr_db(&master);
            let (wave, noise) = match &enc.decoded {
                Some(d) => {
                    let reference = audio::resample(&master.samples, master.rate, enc.rate);
                    let n = d.len().min(reference.len());
                    let diff: Vec<f32> = (0..n).map(|i| (d[i] - reference[i]).abs()).collect();
                    (Some(Wave::of(d)), Some(Wave::of(&diff)))
                }
                None => (None, None),
            };
            Ok(Measured {
                settings,
                rate: enc.rate,
                frames: enc.frames,
                bytes: enc.bytes,
                exact: enc.exact,
                pcm16_bytes: enc.pcm16_bytes(),
                snr,
                footprint: enc.footprint(),
                decoded: enc.decoded.as_deref().map(audio::to_pcm16),
                wave,
                noise,
            })
        })();
        if platform == Platform::Pc {
            insp.master = match masters.first() {
                Some((_, Ok(m))) => {
                    insp.master_wave = Wave::of(&m.samples);
                    Ok((m.rate, audio::to_pcm16(&m.samples)))
                }
                Some((_, Err(e))) => Err(e.clone()),
                None => Err(result.as_ref().err().cloned().unwrap_or_default()),
            };
        }
        insp.columns.push(Column {
            platform,
            declared_codec,
            declared_rate,
            result,
        });
    }
    insp
}

/// One platform's audio, summed over the project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformTotal {
    pub storage: u64,
    pub resident: u64,
    /// Sounds and tracks that stream (the N64 streams everything).
    pub streamed: usize,
    /// The largest stream buffer one playback adds.
    pub stream_buffer: u64,
    /// The same audio as PCM16 at the platform's rates.
    pub pcm16: u64,
    /// Bakes that fail.
    pub errors: usize,
}

/// The project's audio memory per platform (`Platform::ALL` order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub sounds: usize,
    pub platforms: [PlatformTotal; 4],
}

impl Totals {
    pub fn of(&self, p: Platform) -> &PlatformTotal {
        let i = Platform::ALL.iter().position(|q| *q == p).unwrap_or(0);
        &self.platforms[i]
    }
}

/// Every sound and track of the project on every platform, summed.
pub fn totals(assets: &Path) -> Totals {
    let mut t = Totals::default();
    let Ok(manifest) = Manifest::load(&assets.join("manifest.toml")) else {
        return t;
    };
    let entries = manifest
        .sounds
        .iter()
        .map(|(n, d)| (format!("sounds/{n}"), false, d))
        .chain(
            manifest
                .music
                .iter()
                .map(|(n, d)| (format!("music/{n}"), true, d)),
        );
    for (logical, music, decl) in entries {
        t.sounds += 1;
        let id = trino_core::asset_id(&logical);
        for (i, platform) in Platform::ALL.into_iter().enumerate() {
            let enc = (|| -> Result<Encoded, String> {
                let settings = decl.audio_settings(music, platform)?;
                let source = trino_asset_pipeline::resolve_source(assets, platform, &decl.file)?;
                let master = trino_asset_pipeline::decode_master(&source)?;
                audio::encode(platform, &settings, &master, id)
            })();
            let p = &mut t.platforms[i];
            match enc {
                Ok(enc) => {
                    let f = enc.footprint();
                    p.storage += f.bytes as u64;
                    p.resident += f.resident as u64;
                    p.pcm16 += enc.pcm16_bytes() as u64;
                    // Every N64 wav64 plays from the cartridge.
                    if enc.settings.stream || platform == Platform::N64 {
                        p.streamed += 1;
                        p.stream_buffer = p.stream_buffer.max(f.playing as u64);
                    }
                }
                Err(_) => p.errors += 1,
            }
        }
    }
    t
}

/// The budget's line for one platform (the Inspector's platform budget
/// lists all four; the Game view's HUD shows its profile's).
pub fn budget_line(t: &PlatformTotal, p: Platform) -> String {
    use crate::textures::bytes;
    let mut s = format!(
        "Audio {} {} {}",
        platform_label(p),
        bytes(t.storage),
        storage_name(p)
    );
    match p {
        Platform::N64 => s.push_str(" · streams from ROM"),
        _ => s.push_str(&format!(" · {} RAM", bytes(t.resident))),
    }
    if t.streamed > 0 {
        s.push_str(&format!(
            " · {} streamed{}",
            t.streamed,
            if t.stream_buffer > 0 {
                format!(" (+{} each)", bytes(t.stream_buffer))
            } else {
                String::new()
            }
        ));
    }
    s
}

/// Its hover text: what the numbers count and what PCM16 would cost.
pub fn budget_hover(t: &PlatformTotal, p: Platform, sounds: usize) -> String {
    use crate::textures::bytes;
    let ram = match p {
        Platform::N64 => format!(
            "the mixer reads every wav64 from ROM as it plays; {} of RDRAM for the open files",
            bytes(t.resident)
        ),
        Platform::N3ds => format!(
            "{} of linear memory for the sounds that load whole; a stream keeps {} chunks",
            bytes(t.resident),
            audio::N3DS_STREAM_BUFFERS
        ),
        _ => format!(
            "{} for decoded sounds and the encoded files streams decode from; a stream adds a \
             half-second ring",
            bytes(t.resident)
        ),
    };
    format!(
        "{sounds} sounds and tracks: {} in {} ({} as PCM16, {:.1}× smaller); {ram}{}",
        bytes(t.storage),
        storage_name(p),
        bytes(t.pcm16),
        t.pcm16 as f64 / t.storage.max(1) as f64,
        if t.errors > 0 {
            format!("; {} bakes fail: cargo xtask audio report", t.errors)
        } else {
            String::new()
        }
    )
}

/// The handle the preview player uploads to (never a real asset's).
const PREVIEW: SoundId = SoundId(0x7472_6e61);

/// Plays the master or a platform's decoded result on the PC's output (the
/// PC backend: silent without a device), on the sound's bus so the Mixer
/// tab's volumes apply.
pub struct Player {
    pub audio: trino_platform_pc::PcAudio,
    voice: Option<VoiceId>,
    /// What plays: (logical, platform; `None` = the master).
    pub what: Option<(String, Option<Platform>)>,
}

impl Player {
    pub fn new(mix: &MixerConfig) -> Self {
        let mut audio = trino_platform_pc::PcAudio::new();
        mix.apply(&mut audio);
        Player {
            audio,
            voice: None,
            what: None,
        }
    }

    /// Play mono `pcm` at `rate` on `bus` (stopping what played).
    pub fn play(&mut self, what: (String, Option<Platform>), pcm: &[i16], rate: u32, bus: Bus) {
        self.stop();
        let samples = audio::from_pcm16(pcm);
        let samples = trino_platform_pc::audio::resample(&samples, rate, self.audio.sample_rate());
        self.audio.upload_sound(PREVIEW, samples);
        self.voice = self.audio.play_voice(
            PREVIEW,
            &SoundParams {
                bus: Some(bus),
                ..SoundParams::DEFAULT
            },
            false,
        );
        self.what = Some(what);
    }

    pub fn stop(&mut self) {
        if let Some(v) = self.voice.take() {
            self.audio.stop_voice(v);
        }
        self.what = None;
    }

    /// What is still playing (`None` once it ended).
    pub fn playing(&mut self) -> Option<&(String, Option<Platform>)> {
        if self.voice.is_some_and(|v| !self.audio.voice_playing(v)) {
            self.voice = None;
            self.what = None;
        }
        self.what.as_ref()
    }
}

type Job<T> = (Key, Receiver<T>);

/// The audio inspector's state and the budget's totals, with their
/// workers and the preview player.
#[derive(Default)]
pub struct AudioView {
    /// The finished inspection and the key it was measured at.
    pub current: Option<(Key, Inspection)>,
    pending: Option<(String, Job<Inspection>)>,
    pub totals: Option<(Key, Totals)>,
    totals_pending: Option<Job<Totals>>,
    /// The platform the waveform overlays (index into `Platform::ALL`).
    pub preview: usize,
    /// Created on the first Play (opens the output device).
    pub player: Option<Player>,
    /// `quality` being dragged, before it is written.
    pub quality_edit: Option<f32>,
    /// Errors already written to the Console.
    pub logged: std::collections::HashSet<String>,
    /// The caption keys of the sound shown, and the key and sound they were
    /// read at (issue #96).
    caption: Option<(Key, String, Result<CaptionKeys, String>)>,
    /// The "New key…" field (`Some`: open).
    pub new_caption_key: Option<String>,
    /// Where the Preview button shows the caption.
    pub caption_place: trino_core::access::CaptionPlace,
}

impl AudioView {
    /// The caption keys last read (tests, hooks).
    #[cfg(test)]
    pub fn caption_shown(&self) -> Option<&CaptionKeys> {
        self.caption.as_ref().and_then(|(_, _, c)| c.as_ref().ok())
    }

    /// `logical`'s caption keys, re-read when the manifest changes.
    pub fn caption(
        &mut self,
        assets: &Path,
        logical: &str,
        key: Key,
    ) -> Result<CaptionKeys, String> {
        if !self
            .caption
            .as_ref()
            .is_some_and(|(k, l, _)| *k == key && l == logical)
        {
            self.caption = Some((key, logical.to_string(), caption_keys(assets, logical)));
        }
        self.caption
            .as_ref()
            .map_or(Ok(CaptionKeys::default()), |(_, _, c)| c.clone())
    }

    pub fn new(preview: usize) -> Self {
        AudioView {
            preview,
            ..Default::default()
        }
    }

    /// The inspection of `logical`, (re)measured when the key moved.
    /// `None` while the worker runs (it repaints when done). Returns the
    /// errors the Console hasn't shown yet too.
    pub fn inspect(
        &mut self,
        ctx: &egui::Context,
        assets: &Path,
        logical: &str,
        key: Key,
    ) -> (Option<&Inspection>, Vec<String>) {
        let mut new_errors = Vec::new();
        let fresh = self
            .current
            .as_ref()
            .is_some_and(|(k, i)| *k == key && i.logical == logical);
        if !fresh {
            let running = self
                .pending
                .as_ref()
                .is_some_and(|(l, (k, _))| l == logical && *k == key);
            if !running {
                let (tx, rx) = channel();
                let (assets, l, ctx2) = (assets.to_path_buf(), logical.to_string(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(measure(&assets, &l));
                    ctx2.request_repaint();
                });
                self.pending = Some((logical.to_string(), (key, rx)));
            }
            if let Some((_, (k, rx))) = &self.pending {
                match rx.try_recv() {
                    Ok(insp) => {
                        for c in &insp.columns {
                            if let Err(e) = &c.result
                                && self.logged.insert(e.clone())
                            {
                                new_errors.push(format!(
                                    "{} on {}: {e}",
                                    insp.logical,
                                    c.platform.key()
                                ));
                            }
                        }
                        self.current = Some((*k, insp));
                        self.pending = None;
                        self.quality_edit = None;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.pending = None,
                }
            }
        }
        let shown = self
            .current
            .as_ref()
            .filter(|(_, i)| i.logical == logical)
            .map(|(_, i)| i);
        (shown, new_errors)
    }

    /// Is a worker measuring a newer state than the one shown?
    pub fn measuring(&self) -> bool {
        self.pending.is_some()
    }

    /// The project's totals, recomputed on a worker when the key moved
    /// (the last ones stay shown meanwhile).
    pub fn totals(&mut self, ctx: &egui::Context, assets: &Path, key: Key) -> Option<&Totals> {
        let stale = self.totals.as_ref().is_none_or(|(k, _)| *k != key);
        let running = self.totals_pending.as_ref().is_some_and(|(k, _)| *k == key);
        if stale && !running {
            let (tx, rx) = channel();
            let (assets, ctx2) = (assets.to_path_buf(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(totals(&assets));
                ctx2.request_repaint();
            });
            self.totals_pending = Some((key, rx));
        }
        if let Some((k, rx)) = &self.totals_pending {
            match rx.try_recv() {
                Ok(t) => {
                    self.totals = Some((*k, t));
                    self.totals_pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.totals_pending = None,
            }
        }
        self.totals.as_ref().map(|(_, t)| t)
    }
}

/// The project mix the Mixer tab edits (`assets/audio.toml`).
pub struct MixState {
    /// What the tab shows (edits land here first).
    pub file: MixFile,
    /// `assets/audio.toml` exists (else the first edit creates it).
    pub exists: bool,
    /// Why the file on disk, or the edit, isn't a valid mix.
    pub error: Option<String>,
    /// Edited and not written yet (written once the user settles).
    pub dirty: bool,
    /// Preview-only mutes (the mix file has none: players mute in the
    /// game's options, which persist apart).
    pub muted: [bool; Bus::COUNT],
}

impl MixState {
    pub fn path(assets: &Path) -> PathBuf {
        assets.join(MixFile::FILE)
    }

    /// The mix of the project at `assets` (an empty one, every bus at 1,
    /// when there is no file yet).
    pub fn load(assets: &Path) -> Self {
        let mut s = MixState {
            file: MixFile {
                version: 1,
                ..Default::default()
            },
            exists: false,
            error: None,
            dirty: false,
            muted: [false; Bus::COUNT],
        };
        s.reload(std::fs::read_to_string(Self::path(assets)).ok().as_deref());
        s
    }

    /// The file changed on disk (`None`: removed): show it, or why it
    /// doesn't parse (keeping the last good mix).
    pub fn reload(&mut self, text: Option<&str>) {
        self.exists = text.is_some();
        self.dirty = false;
        match text.map(MixFile::parse) {
            Some(Ok(f)) => {
                self.file = f;
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(format!("{}: {e}", MixFile::FILE)),
            None => {
                self.file = MixFile {
                    version: 1,
                    ..Default::default()
                };
                self.error = None;
            }
        }
    }

    pub fn volume(&self, bus: Bus) -> f32 {
        self.file.buses.get(bus.label()).copied().unwrap_or(1.0)
    }

    pub fn set_volume(&mut self, bus: Bus, v: f32) {
        self.file.buses.insert(
            bus.label().into(),
            (v.clamp(0.0, 1.0) * 100.0).round() / 100.0,
        );
        self.dirty = true;
    }

    /// The mix as the backends apply it (the default mix when the edit
    /// doesn't validate), with the preview mutes.
    pub fn config(&self) -> MixerConfig {
        self.file.config().unwrap_or_default()
    }

    /// The text to write, or why the edit isn't a valid mix.
    pub fn to_text(&self) -> Result<String, String> {
        self.file.config()?;
        let text = self.file.to_toml();
        MixFile::parse(&text)?;
        Ok(text)
    }

    /// Apply the mix and the preview mutes to the preview player.
    pub fn apply(&self, audio: &mut dyn Audio) {
        self.config().apply(audio);
        for b in Bus::ALL {
            audio.set_bus_muted(b, self.muted[b.index()]);
        }
    }
}

/// `logical`'s caption keys in the manifest under `assets` (issue #96).
pub fn caption_keys(assets: &Path, logical: &str) -> Result<CaptionKeys, String> {
    let manifest = Manifest::load(&assets.join("manifest.toml"))?;
    let (music, name) = split(logical)?;
    let table = if music {
        &manifest.music
    } else {
        &manifest.sounds
    };
    let decl = table
        .get(name)
        .ok_or_else(|| format!("`{logical}` is not in the manifest"))?;
    Ok(CaptionKeys {
        key: decl.caption.clone(),
        important: decl.important,
        priority: decl.caption_priority,
        direction: decl.direction,
        seconds: decl.caption_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    /// The engine's theme on every platform: each default codec, its
    /// size against PCM16 and its SNR over the issue's floor; the N64
    /// streams it from ROM, the 3DS and the PC stream it in chunks/rings.
    #[test]
    fn the_theme_is_measured_like_the_bake() {
        let insp = measure(&assets(), "music/theme");
        let col = |p| {
            insp.columns
                .iter()
                .find(|c| c.platform == p)
                .unwrap()
                .result
                .as_ref()
                .unwrap()
        };
        let (rate, master) = insp.master.as_ref().unwrap();
        assert!(*rate > 0 && !master.is_empty());
        for (p, codec) in [
            (Platform::Pc, Codec::Vorbis),
            (Platform::N64, Codec::Vadpcm),
            (Platform::N3ds, Codec::DspAdpcm),
            (Platform::Web, Codec::Vorbis),
        ] {
            let m = col(p);
            assert_eq!((m.settings.codec, m.settings.auto), (codec, true), "{p:?}");
            assert!(m.ratio() > 2.5, "{p:?} {codec:?}: {:.1}×", m.ratio());
            // Every default codec keeps its floor on the theme.
            let snr = m.snr.unwrap();
            assert!(snr >= snr_floor(codec).unwrap(), "{p:?}: {snr:.1} dB");
            assert!(m.settings.stream, "music streams");
            assert_eq!(m.wave.as_ref().unwrap().min.len(), WAVE_BUCKETS);
        }
        assert!(col(Platform::N64).footprint.resident < 2048, "from ROM");
        assert_eq!(col(Platform::N3ds).footprint.playing, 3 * 8192);
        assert_eq!(col(Platform::Pc).footprint.playing, audio::PC_STREAM_BYTES);
        // The self-test's Opus track: no host decode, an estimated size.
        let opus = measure(&assets(), "music/selftest_opus");
        let n64 = opus
            .columns
            .iter()
            .find(|c| c.platform == Platform::N64)
            .unwrap();
        let m = n64.result.as_ref().unwrap();
        assert_eq!(n64.declared_codec.as_deref(), Some("opus"));
        assert!(!m.exact && m.snr.is_none() && m.decoded.is_none());
        // Not an audio asset, or not in the manifest: every column says so.
        assert!(measure(&assets(), "sprites/x").columns[0].result.is_err());
        assert!(
            measure(&assets(), "sounds/ghost").columns[3]
                .result
                .is_err()
        );
    }

    #[test]
    fn totals_count_every_sound_and_the_budget_lines_read() {
        let t = totals(&assets());
        assert!(t.sounds >= 10, "{}", t.sounds);
        let n64 = t.of(Platform::N64);
        assert!(n64.storage > 0 && n64.errors == 0);
        assert_eq!(n64.resident, (t.sounds * audio::N64_OPEN_BYTES) as u64);
        assert_eq!(n64.streamed, t.sounds, "the N64 streams everything");
        let pc = t.of(Platform::Pc);
        assert!(pc.storage < pc.pcm16, "Vorbis by default");
        let line = budget_line(n64, Platform::N64);
        assert!(
            line.starts_with("Audio N64 ") && line.contains("ROM"),
            "{line}"
        );
        let line = budget_line(t.of(Platform::N3ds), Platform::N3ds);
        assert!(line.contains("RomFS") && line.contains("RAM"), "{line}");
        assert!(budget_hover(pc, Platform::Pc, t.sounds).contains("as PCM16"));
    }

    #[test]
    fn waves_bucket_min_and_max() {
        let w = Wave::of(&[0.5, -0.25, 1.0, 0.0]);
        assert_eq!(w.min, [0.0, -0.25, 0.0, 0.0]);
        assert_eq!(w.max, [0.5, 0.0, 1.0, 0.0]);
        let long: Vec<f32> = (0..10_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let w = Wave::of(&long);
        assert_eq!(w.max.len(), WAVE_BUCKETS);
        assert!(w.max.iter().all(|&m| m <= 1.0) && w.min.iter().any(|&m| m < -0.9));
        assert!(Wave::of(&[]).min.len() == 1);
    }

    #[test]
    fn the_mix_reloads_validates_and_writes_text_that_parses() {
        let mut m = MixState {
            file: MixFile {
                version: 1,
                ..Default::default()
            },
            exists: false,
            error: None,
            dirty: false,
            muted: [false; Bus::COUNT],
        };
        assert_eq!(m.volume(Bus::Music), 1.0);
        m.set_volume(Bus::Music, 0.734);
        assert!(m.dirty);
        let text = m.to_text().unwrap();
        assert!(text.contains("music = 0.73"), "{text}");
        assert_eq!(MixFile::parse(&text).unwrap().buses["music"], 0.73);
        m.file.duck.push(audio::DuckDecl {
            trigger: "music".into(),
            target: "music".into(),
            level: 0.5,
            attack_ms: 0,
            release_ms: 0,
            comment: String::new(),
        });
        assert!(m.to_text().is_err(), "a bus can't duck itself");
        m.reload(Some("version = 1\n[buses]\nsfx = 0.5\n"));
        assert!(!m.dirty && m.exists && m.error.is_none());
        assert_eq!(m.volume(Bus::Sfx), 0.5);
        m.reload(Some("version = 7\n"));
        assert!(
            m.error.is_some() && m.volume(Bus::Sfx) == 0.5,
            "keeps the last good mix"
        );
        m.reload(None);
        assert!(!m.exists && m.volume(Bus::Sfx) == 1.0);
    }
}
