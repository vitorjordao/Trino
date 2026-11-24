//! Listening evidence for pitch, music control, bus filters and positional
//! voices (Bongo-Kart #175, #177, #179; ADR 0044): the PC backend renders
//! each feature offline into `target/evidence/audio-control/*.wav`, from
//! the engine's own baked sounds where there is one, and prints the
//! mixer's cost with and without pitch.
//!
//! `cargo test --release -p trino-platform-pc --test audio_evidence -- --ignored --nocapture`

use std::path::PathBuf;
use std::time::Instant;

use trino_core::{
    Audio, Bus, BusFilter, Doppler, Listener, MusicId, SoundEmitter, SoundId, SoundParams, Vec3,
    VoicePool,
};
use trino_platform_pc::PcAudio;

const RATE: u32 = 48_000;
/// One game frame of output.
const FRAME: usize = RATE as usize / 60;

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/audio-control");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 16-bit stereo WAV.
fn write_wav(name: &str, stereo: &[f32]) {
    let data: Vec<u8> = stereo
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut b = Vec::with_capacity(44 + data.len());
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let path = out_dir().join(name);
    std::fs::write(&path, b).unwrap();
    println!("wrote {}", path.display());
}

/// The engine's sounds and music, baked for the PC.
fn engine_audio() -> PcAudio {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut audio = PcAudio::new_offline(RATE);
    for s in assets.sounds {
        audio.upload_sound_blob(SoundId(s.id), &s.blob).unwrap();
    }
    for m in assets.music {
        audio.upload_music_blob(MusicId(m.id), &m.blob).unwrap();
    }
    audio
}

/// Render `seconds` a frame at a time, calling `each` before every frame
/// with the frame's time.
fn run(a: &mut PcAudio, out: &mut Vec<f32>, seconds: f32, mut each: impl FnMut(&mut PcAudio, f32)) {
    let frames = (seconds * 60.0).round() as usize;
    let mut buf = vec![0.0f32; FRAME * 2];
    for i in 0..frames {
        each(a, i as f32 / 60.0);
        a.render(&mut buf);
        out.extend_from_slice(&buf);
    }
}

/// A looping synthesized part at `RATE`: `notes` (Hz, 0 = rest) of `beat`
/// seconds each, through `voice` (a phase in turns → a sample).
fn part(notes: &[f32], beat: f32, gain: f32, voice: fn(f32) -> f32) -> Vec<f32> {
    let per = (beat * RATE as f32) as usize;
    let mut out = Vec::with_capacity(per * notes.len());
    for &hz in notes {
        let mut phase = 0.0f32;
        for i in 0..per {
            let env = (1.0 - i as f32 / per as f32).powf(0.6);
            out.push(if hz > 0.0 {
                voice(phase) * env * gain
            } else {
                0.0
            });
            phase = (phase + hz / RATE as f32).fract();
        }
    }
    out
}

fn sine(p: f32) -> f32 {
    (p * std::f32::consts::TAU).sin()
}

fn saw(p: f32) -> f32 {
    2.0 * p - 1.0
}

/// #175: the same splash at 0.5×, 0.75×, 1×, 1.5× and 2×, one after the
/// other; then a voice bent from 1× to 2× while it plays.
#[test]
#[ignore = "writes listening evidence"]
fn pitch() {
    let mut a = engine_audio();
    let splash = SoundId::from_path("sounds/water_splash");
    let mut out = Vec::new();
    for p in [0.5, 0.75, 1.0, 1.5, 2.0] {
        a.play_sound_with(
            splash,
            &SoundParams {
                pitch: p,
                ..SoundParams::DEFAULT
            },
        );
        run(&mut a, &mut out, 1.2, |_, _| {});
    }
    let water = SoundId::from_path("sounds/water");
    let v = a.play_voice(water, &SoundParams::DEFAULT, true).unwrap();
    run(&mut a, &mut out, 3.0, |a, t| {
        a.set_voice_pitch(v, 1.0 + t / 3.0)
    });
    a.stop_voice(v);
    write_wav("pitch.wav", &out);
}

/// #177: the theme plays 2 s, pauses 1 s, goes on where it was for 2 s.
#[test]
#[ignore = "writes listening evidence"]
fn pause_and_resume() {
    let mut a = engine_audio();
    let mut out = Vec::new();
    a.play_music(MusicId::from_path("music/theme"), true);
    run(&mut a, &mut out, 2.0, |_, _| {});
    let held = a.music_position().unwrap();
    a.pause_music();
    run(&mut a, &mut out, 1.0, |_, _| {});
    assert_eq!(a.music_position(), Some(held));
    a.resume_music();
    run(&mut a, &mut out, 2.0, |_, _| {});
    // The theme is a 3.2 s loop: 2 s after resuming it reads 0.8 s.
    let now = a.music_position().unwrap();
    println!("paused at {held:.3} s; 2 s after resuming at {now:.3} s of the 3.2 s loop");
    assert!((now - (held + 2.0) % 3.2).abs() < 0.01);
    write_wav("pause_resume.wav", &out);
}

/// #177: a synthesized 4-bar loop, then a 1.5 s crossfade in sync to its
/// variation (same tempo: the beat doesn't skip), then a cut back.
#[test]
#[ignore = "writes listening evidence"]
fn crossfade() {
    let mut a = PcAudio::new_offline(RATE);
    let calm = [220.0, 0.0, 261.6, 0.0, 293.7, 0.0, 261.6, 0.0];
    let busy = [440.0, 523.3, 587.3, 523.3, 659.3, 587.3, 523.3, 493.9];
    a.upload_music(MusicId(1), part(&calm, 0.25, 0.4, sine));
    a.upload_music(MusicId(2), part(&busy, 0.25, 0.25, saw));
    let mut out = Vec::new();
    a.play_music(MusicId(1), true);
    run(&mut a, &mut out, 3.0, |_, _| {});
    a.crossfade_music(MusicId(2), true, 1500, true);
    run(&mut a, &mut out, 4.0, |_, _| {});
    a.crossfade_music(MusicId(1), true, 0, true);
    run(&mut a, &mut out, 2.0, |_, _| {});
    write_wav("crossfade.wav", &out);
}

/// #177: three layers of one loop: bass alone, the melody fades in over a
/// second, the arpeggio after, then both fade out.
#[test]
#[ignore = "writes listening evidence"]
fn layers() {
    let mut a = PcAudio::new_offline(RATE);
    let bass = [110.0, 0.0, 110.0, 0.0, 146.8, 0.0, 130.8, 0.0];
    let melody = [440.0, 0.0, 523.3, 0.0, 587.3, 523.3, 440.0, 0.0];
    let arp = [880.0, 1046.5, 1318.5, 1046.5, 880.0, 1046.5, 1318.5, 1568.0];
    a.upload_music(MusicId(1), part(&bass, 0.25, 0.5, sine));
    a.upload_music(MusicId(2), part(&melody, 0.25, 0.25, saw));
    a.upload_music(MusicId(3), part(&arp, 0.25, 0.15, sine));
    let mut out = Vec::new();
    a.play_music_layers(&[MusicId(1), MusicId(2), MusicId(3)], true);
    run(&mut a, &mut out, 2.0, |_, _| {});
    a.set_music_layer(1, 1.0, 1000);
    run(&mut a, &mut out, 2.0, |_, _| {});
    a.set_music_layer(2, 1.0, 1000);
    run(&mut a, &mut out, 2.0, |_, _| {});
    a.set_music_layer(1, 0.0, 1000);
    a.set_music_layer(2, 0.0, 1000);
    run(&mut a, &mut out, 2.0, |_, _| {});
    write_wav("layers.wav", &out);
}

/// #177: the theme on the music bus, a 600 Hz low-pass for 2 s (under
/// water, a pause menu), then off again.
#[test]
#[ignore = "writes listening evidence"]
fn bus_low_pass() {
    let mut a = engine_audio();
    let mut out = Vec::new();
    a.play_music(MusicId::from_path("music/theme"), true);
    run(&mut a, &mut out, 2.0, |_, _| {});
    a.set_bus_filter(Bus::Music, Some(BusFilter::LowPass { cutoff_hz: 600.0 }));
    run(&mut a, &mut out, 2.0, |_, _| {});
    a.set_bus_filter(Bus::Music, None);
    run(&mut a, &mut out, 2.0, |_, _| {});
    write_wav("bus_low_pass.wav", &out);
}

/// #179: a 30 m/s engine hum passing a still listener 4 m away through a
/// `VoicePool` (physical doppler: up a semitone and a half, then down; pan
/// left to right, louder as it nears).
#[test]
#[ignore = "writes listening evidence"]
fn doppler_pass_by() {
    let mut a = PcAudio::new_offline(RATE);
    let hum: Vec<f32> = (0..RATE as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            0.3 * saw((t * 110.0).fract()) + 0.2 * sine((t * 220.0).fract())
        })
        .collect();
    a.upload_sound(SoundId(1), hum);
    let mut pool = VoicePool::<2>::new().with_doppler(Doppler::PHYSICAL);
    let listener = Listener {
        pos: Vec3::ZERO,
        right: Vec3::new(1.0, 0.0, 0.0),
    };
    let speed = 30.0;
    let start = Vec3::new(-60.0, 0.0, 4.0);
    let kart = SoundEmitter {
        looped: true,
        vel: Vec3::new(speed, 0.0, 0.0),
        ..SoundEmitter::new(SoundId(1), start, 70.0)
    };
    pool.set_listener(listener, Vec3::ZERO);
    let v = pool.play_voice_at(&mut a, &kart).unwrap();
    let mut out = Vec::new();
    run(&mut a, &mut out, 4.0, |a, t| {
        let pos = Vec3::new(start.x + speed * t, 0.0, 4.0);
        pool.move_voice(v, pos, kart.vel);
        pool.update(a);
    });
    let approach =
        Doppler::PHYSICAL.pitch(&listener, Vec3::ZERO, Vec3::new(-40.0, 0.0, 4.0), kart.vel);
    let away = Doppler::PHYSICAL.pitch(&listener, Vec3::ZERO, Vec3::new(40.0, 0.0, 4.0), kart.vel);
    println!(
        "doppler at 30 m/s: {approach:.3} approaching, {away:.3} leaving ({:.2} semitones apart)",
        12.0 * (approach / away).log2()
    );
    pool.stop_all(&mut a);
    write_wav("doppler_pass_by.wav", &out);
}

/// The mixer's cost: 24 voices (the PC's limit) of noise for 10 s of output at pitch 1 (the
/// whole-sample path) and at a fractional pitch (interpolated), plus the
/// same with a low-pass on their bus.
#[test]
#[ignore = "measures the mixer"]
fn mixer_cost() {
    let noise: Vec<f32> = (0..RATE as usize)
        .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 8) as f32 / (1 << 24) as f32 - 0.5)
        .collect();
    for (label, pitch, filter) in [
        ("pitch 1", 1.0, false),
        ("pitch 1.37", 1.37, false),
        ("pitch 1 + low-pass", 1.0, true),
        ("pitch 1.37 + low-pass", 1.37, true),
    ] {
        let mut a = PcAudio::new_offline(RATE);
        a.upload_sound(SoundId(1), noise.clone());
        if filter {
            a.set_bus_filter(Bus::Sfx, Some(BusFilter::LowPass { cutoff_hz: 900.0 }));
        }
        for _ in 0..24 {
            a.play_voice(
                SoundId(1),
                &SoundParams {
                    pitch,
                    volume: 0.03,
                    ..SoundParams::DEFAULT
                },
                true,
            )
            .unwrap();
        }
        let mut buf = vec![0.0f32; 512 * 2];
        let blocks = 10 * RATE as usize / 512;
        let t = Instant::now();
        for _ in 0..blocks {
            a.render(&mut buf);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "TRINO_MIX_COST: {label}: 24 voices, 10 s of output in {ms:.1} ms ({:.3} % of real time)",
            ms / 100.0
        );
    }
}
