//! Pitch, music control and bus filters through the `Audio` trait on the PC
//! backend (Bongo-Kart #175, #177; ADR 0044), rendered offline: what a game
//! calls, sample by sample.

use trino_asset_pipeline::Platform;
use trino_asset_pipeline::audio::{AudioSettings, Codec, Master};
use trino_core::{
    Audio, AudioFeatures, Bus, BusFilter, MAX_MUSIC_LAYERS, MusicId, SoundId, SoundParams,
};
use trino_platform_pc::PcAudio;

const RATE: u32 = 8_000;
const STEP: usize = 64;

fn offline() -> PcAudio {
    PcAudio::new_offline(RATE)
}

/// Left channel of `frames` rendered frames, a device buffer at a time
/// (offline, streams refill between buffers: one huge buffer would drain
/// their half-second ring).
fn render(a: &mut PcAudio, frames: usize) -> Vec<f32> {
    let mut left = Vec::with_capacity(frames);
    let mut out = vec![0.0f32; 256 * 2];
    while left.len() < frames {
        let n = (frames - left.len()).min(256);
        a.render(&mut out[..n * 2]);
        left.extend(out[..n * 2].chunks(2).map(|f| f[0]));
    }
    left
}

/// Frames until `voice` stops playing, rendering `STEP` at a time.
fn length_of(a: &mut PcAudio, voice: trino_core::VoiceId) -> usize {
    let mut n = 0;
    while a.voice_playing(voice) {
        render(a, STEP);
        n += STEP;
        assert!(n < RATE as usize * 60, "never ends");
    }
    n
}

#[test]
fn the_pc_reports_every_feature() {
    let a = offline();
    assert_eq!(a.audio_features(), AudioFeatures::ALL);
    assert_eq!(a.audio_features().music_layers as usize, MAX_MUSIC_LAYERS);
}

/// The issue's acceptance: pitching one voice never changes another's
/// duration; 2× ends in half the time, 0.5× in twice.
#[test]
fn pitch_is_per_voice_and_scales_its_duration() {
    let mut a = offline();
    a.upload_sound(SoundId(1), vec![0.2; RATE as usize]); // 1 s
    let at = |pitch: f32| SoundParams {
        pitch,
        ..SoundParams::DEFAULT
    };
    let lone = {
        let v = a.play_voice(SoundId(1), &at(1.0), false).unwrap();
        length_of(&mut a, v)
    };
    let fast = a.play_voice(SoundId(1), &at(2.0), false).unwrap();
    let plain = a.play_voice(SoundId(1), &at(1.0), false).unwrap();
    let slow = a.play_voice(SoundId(1), &at(0.5), false).unwrap();
    let (mut ends, mut n) = ([0usize; 3], 0);
    while a.voice_playing(slow) {
        render(&mut a, STEP);
        n += STEP;
        for (i, v) in [fast, plain, slow].into_iter().enumerate() {
            if ends[i] == 0 && !a.voice_playing(v) {
                ends[i] = n;
            }
        }
    }
    assert_eq!(ends[1], lone, "the plain voice kept its length");
    assert!(ends[0].abs_diff(lone / 2) <= STEP, "{ends:?} vs {lone}");
    assert!(ends[2].abs_diff(lone * 2) <= STEP, "{ends:?} vs {lone}");
    // Out of range and NaN: clamped, never stuck or silent.
    let wild = a.play_voice(SoundId(1), &at(9.0), false).unwrap();
    assert!(length_of(&mut a, wild).abs_diff(lone / 2) <= STEP);
    let nan = a.play_voice(SoundId(1), &at(f32::NAN), false).unwrap();
    assert_eq!(length_of(&mut a, nan), lone);
}

#[test]
fn set_voice_pitch_bends_a_playing_voice() {
    let mut a = offline();
    a.upload_sound(SoundId(1), vec![0.2; RATE as usize]);
    let v = a
        .play_voice(SoundId(1), &SoundParams::DEFAULT, false)
        .unwrap();
    render(&mut a, RATE as usize / 2); // half played
    a.set_voice_pitch(v, 2.0);
    let rest = length_of(&mut a, v);
    assert!(rest.abs_diff(RATE as usize / 4) <= STEP, "{rest}");
    // set_voice carries the pitch too.
    let v = a
        .play_voice(SoundId(1), &SoundParams::DEFAULT, false)
        .unwrap();
    a.set_voice(
        v,
        &SoundParams {
            pitch: 0.5,
            ..SoundParams::DEFAULT
        },
    );
    let n = length_of(&mut a, v);
    assert!(n.abs_diff(RATE as usize * 2) <= STEP, "{n}");
}

/// A ramp 0..1 over `n` samples: a sample's value says where it was read.
fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 / n as f32).collect()
}

#[test]
fn music_pauses_where_it_is_and_reports_its_position() {
    let mut a = offline();
    a.upload_music(MusicId(1), ramp(RATE as usize));
    assert_eq!(a.music_position(), None);
    a.play_music(MusicId(1), true);
    render(&mut a, RATE as usize / 4);
    let p = a.music_position().unwrap();
    assert!((p - 0.25).abs() < 1e-3, "{p}");
    a.pause_music();
    assert!(a.music_paused());
    let quiet = render(&mut a, RATE as usize / 4);
    assert!(quiet.iter().all(|&s| s == 0.0), "silent while paused");
    assert_eq!(a.music_position(), Some(p), "held");
    a.resume_music();
    assert!(!a.music_paused());
    let next = render(&mut a, 1)[0];
    assert!((next - 0.25).abs() < 1e-3, "goes on from there: {next}");
    // A loop's position wraps.
    render(&mut a, RATE as usize);
    let p = a.music_position().unwrap();
    assert!(p < 1.0 && (p - 0.25).abs() < 1e-2, "{p}");
}

#[test]
fn a_synced_crossfade_starts_the_new_track_where_the_old_one_was() {
    let mut a = offline();
    let n = RATE as usize;
    a.upload_music(MusicId(1), ramp(n));
    a.upload_music(MusicId(2), ramp(n).into_iter().map(|v| -v).collect());
    a.play_music(MusicId(1), true);
    render(&mut a, n / 2);
    // A cut (0 ms) in sync: the next sample is the other track's at 0.5.
    a.crossfade_music(MusicId(2), true, 0, true);
    let s = render(&mut a, 1)[0];
    assert!((s + 0.5).abs() < 1e-3, "{s}");
    // A 100 ms fade from the start: both tracks, then the new one alone.
    a.play_music(MusicId(1), true);
    render(&mut a, n / 4);
    a.crossfade_music(MusicId(2), true, 100, false);
    let mid = render(&mut a, n / 20); // 50 ms
    let half = mid[mid.len() - 1];
    // Old at ~0.30 × ~0.5, new at ~0.05 × -0.5: in between.
    assert!(half > -0.1 && half < 0.2, "{half}");
    let after = render(&mut a, n / 10);
    let last = after[after.len() - 1];
    let pos = a.music_position().unwrap();
    assert!(
        (last + pos).abs() < 2e-3,
        "only the new track: {last} at {pos}"
    );
}

/// A crossfade to a streamed (Vorbis) track in sync: the page seek lands it
/// on the old track's sample.
#[test]
fn a_synced_crossfade_seeks_a_stream() {
    let n = RATE as usize * 6;
    let tone = |k: f32| -> Vec<f32> {
        (0..n)
            .map(|i| 0.4 * (i as f32 * k * std::f32::consts::TAU / RATE as f32).sin())
            .collect()
    };
    let blob = |samples: Vec<f32>| {
        let settings = AudioSettings {
            codec: Codec::Vorbis,
            auto: false,
            quality: 4.0,
            sample_rate: None,
            stream: true,
            looped: true,
            bus: Bus::Music,
        };
        let master = Master {
            rate: RATE,
            samples,
        };
        trino_asset_pipeline::audio::encode(Platform::Pc, &settings, &master, 1)
            .unwrap()
            .blob
            .unwrap()
    };
    let mut a = offline();
    a.upload_music_blob(MusicId(1), &blob(tone(220.0))).unwrap();
    a.upload_music_blob(MusicId(2), &blob(tone(330.0))).unwrap();
    // The reference: track 2 from the top.
    a.play_music(MusicId(2), true);
    let whole = render(&mut a, n);
    a.play_music(MusicId(1), true);
    let at = RATE as usize * 4 + 123;
    render(&mut a, at);
    a.crossfade_music(MusicId(2), true, 0, true);
    let got = render(&mut a, 1024);
    let worst = got
        .iter()
        .zip(&whole[at..])
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "off by {worst}");
    let p = a.music_position().unwrap();
    assert!((p - (at + 1024) as f32 / RATE as f32).abs() < 1e-3, "{p}");
}

#[test]
fn layers_play_together_and_fade_in_and_out() {
    let mut a = offline();
    let n = RATE as usize;
    a.upload_music(MusicId(1), vec![0.1; n]);
    a.upload_music(MusicId(2), vec![0.2; n]);
    a.upload_music(MusicId(3), vec![0.4; n]);
    a.play_music_layers(&[MusicId(1), MusicId(2), MusicId(3)], true);
    assert_eq!(a.music_layer(0), Some(1.0));
    assert_eq!(a.music_layer(1), Some(0.0));
    assert_eq!(a.music_layer(3), None);
    let s = render(&mut a, 4);
    assert!((s[3] - 0.1).abs() < 1e-6, "layer 0 alone: {s:?}");
    a.set_music_layer(2, 1.0, 0);
    let s = render(&mut a, 4);
    assert!((s[3] - 0.5).abs() < 1e-6, "0 and 2: {s:?}");
    a.set_music_layer(1, 1.0, 1000);
    render(&mut a, n / 2);
    let g = a.music_layer(1).unwrap();
    assert!((g - 0.5).abs() < 0.02, "half way up: {g}");
    render(&mut a, n);
    assert_eq!(a.music_layer(1), Some(1.0));
    let s = render(&mut a, 4);
    assert!((s[3] - 0.7).abs() < 1e-5, "all three: {s:?}");
    // Every layer pauses and resumes together, in step.
    a.pause_music();
    assert!(render(&mut a, 8).iter().all(|&v| v == 0.0));
    a.resume_music();
    assert!((render(&mut a, 1)[0] - 0.7).abs() < 1e-5);
    // Plain music replaces the layers.
    a.play_music(MusicId(1), true);
    assert_eq!(a.music_layer(1), None);
}

#[test]
fn a_bus_filter_is_set_read_and_taken_off() {
    let mut a = offline();
    let lp = BusFilter::LowPass { cutoff_hz: 500.0 };
    assert_eq!(a.bus_filter(Bus::Sfx), None);
    a.set_bus_filter(Bus::Sfx, Some(lp));
    assert_eq!(a.bus_filter(Bus::Sfx), Some(lp));
    let hiss: Vec<f32> = (0..RATE as usize)
        .map(|i| if i % 2 == 0 { 0.3 } else { -0.3 })
        .collect();
    a.upload_sound(SoundId(1), hiss);
    a.play_sound(SoundId(1));
    let s = render(&mut a, RATE as usize / 2);
    let rms = (s[400..].iter().map(|v| v * v).sum::<f32>() / (s.len() - 400) as f32).sqrt();
    assert!(rms < 0.01, "Nyquist hiss cut: {rms}");
    a.set_bus_filter(Bus::Sfx, None);
    assert_eq!(a.bus_filter(Bus::Sfx), None);
    let s = render(&mut a, 4);
    assert!(s.iter().all(|v| (v.abs() - 0.3).abs() < 1e-6), "{s:?}");
}
