//! Audio v2 through the public API: the trait's provided methods on a
//! backend that only implements the four basics, positional audio
//! (`Listener` + `spatialize`) and `Ambience` steering a voice-limited
//! mixer as a listener walks past sound sources.

mod common;

use common::MockAudio;
use trino_core::render3d::Camera3;
use trino_core::{
    Ambience, Audio, Listener, MusicId, SoundId, SoundParams, SoundSource, Vec3, VoiceId,
    spatialize,
};

/// The original four-method backend.
#[derive(Default)]
struct Basic {
    played: Vec<SoundId>,
    music: Option<(MusicId, bool)>,
    volume: f32,
}

impl Audio for Basic {
    fn play_sound(&mut self, sound: SoundId) {
        self.played.push(sound);
    }
    fn play_music(&mut self, music: MusicId, looped: bool) {
        self.music = Some((music, looped));
    }
    fn stop_music(&mut self) {
        self.music = None;
    }
    fn set_master_volume(&mut self, volume: f32) {
        self.volume = volume;
    }
}

#[test]
fn provided_audio_methods_degrade_to_the_basics() {
    let mut a = Basic::default();
    let beep = SoundId::from_path("sounds/beep");
    let dyn_a: &mut dyn Audio = &mut a;
    // A positional one-shot still plays (unpanned) on a basic backend.
    dyn_a.play_sound_with(
        beep,
        &SoundParams {
            volume: 0.2,
            pan: -1.0,
            ..SoundParams::DEFAULT
        },
    );
    // No voices: steering calls are no-ops and nothing reports playing.
    assert_eq!(dyn_a.play_voice(beep, &SoundParams::DEFAULT, true), None);
    dyn_a.set_voice(VoiceId(1), &SoundParams::DEFAULT);
    dyn_a.stop_voice(VoiceId(1));
    assert!(!dyn_a.voice_playing(VoiceId(1)));
    dyn_a.play_stinger(MusicId::from_path("music/fanfare"));
    assert!(!dyn_a.stinger_playing());
    dyn_a.set_music_volume(0.5);
    dyn_a.play_music(MusicId::from_path("music/theme"), true);
    dyn_a.set_master_volume(0.75);
    assert_eq!(a.played, [beep]);
    assert_eq!(a.music, Some((MusicId::from_path("music/theme"), true)));
    assert_eq!(a.volume, 0.75);
    a.stop_music();
    assert_eq!(a.music, None);
}

/// The bus API (issue #53) on the original four-method backend: the master
/// bus is `set_master_volume`, the rest are documented no-ops that read
/// back as "full volume, unmuted, not ducked, nothing counted".
#[test]
fn bus_methods_degrade_to_the_basics() {
    use trino_core::{AudioStats, Bus, BusSettings, DuckRule, MixerConfig};
    let mut a = Basic::default();
    let dyn_a: &mut dyn Audio = &mut a;
    dyn_a.set_bus_volume(Bus::Master, 0.3);
    dyn_a.set_bus_volume(Bus::Music, 0.2); // set_music_volume: a no-op here
    dyn_a.set_bus_volume(Bus::Voice, 0.1);
    for b in Bus::ALL {
        assert_eq!(dyn_a.bus_volume(b), 1.0, "{b:?}");
        dyn_a.set_bus_muted(b, true);
        assert!(!dyn_a.bus_muted(b));
        assert_eq!(dyn_a.bus_duck(b), 1.0);
    }
    dyn_a.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300));
    dyn_a.clear_ducks();
    assert_eq!(dyn_a.audio_stats(), AudioStats::default());
    // A project mix and saved settings apply through the same calls.
    let mut mix = MixerConfig::default();
    mix.volumes[Bus::Master.index()] = 0.6;
    mix.apply(dyn_a);
    BusSettings::default().apply(dyn_a);
    assert_eq!(a.volume, 1.0, "the saved settings won over the mix");
    // A sound sent to a bus is still the same sound on a basic backend.
    let beep = SoundId::from_path("sounds/beep");
    a.play_sound_with(beep, &SoundParams::DEFAULT.on(Bus::Ui));
    assert_eq!(a.played, [beep]);
}

#[test]
fn voices_and_stingers_on_a_mixing_backend() {
    let mut a = MockAudio::new(2);
    let water = SoundId::from_path("sounds/water");
    let theme = MusicId::from_path("music/theme");
    a.play_music(theme, true);
    a.set_music_volume(0.8);
    assert!((a.effective_music_volume() - 0.8).abs() < 1e-6);
    a.play_stinger(MusicId::from_path("music/fanfare"));
    assert!(a.stinger_playing());
    assert!(
        (a.effective_music_volume() - 0.24).abs() < 1e-6,
        "music ducks under a stinger"
    );
    a.finish_stinger();
    assert!(!a.stinger_playing());
    assert!(
        (a.effective_music_volume() - 0.8).abs() < 1e-6,
        "and comes back"
    );

    let v1 = a.play_voice(water, &SoundParams::DEFAULT, true).unwrap();
    let v2 = a.play_voice(water, &SoundParams::DEFAULT, false).unwrap();
    assert_ne!(v1, v2);
    assert_eq!(
        a.play_voice(water, &SoundParams::DEFAULT, true),
        None,
        "voice budget exhausted"
    );
    let quiet = SoundParams {
        volume: 0.1,
        pan: 0.5,
        ..SoundParams::DEFAULT
    };
    a.set_voice(v1, &quiet);
    assert_eq!(a.voice(v1).unwrap().params, quiet);
    assert!(a.voice(v1).unwrap().looped && !a.voice(v2).unwrap().looped);
    a.stop_voice(v1);
    assert!(!a.voice_playing(v1) && a.voice_playing(v2));
    assert!(a.play_voice(water, &SoundParams::DEFAULT, true).is_some());
}

#[test]
fn gains_split_volume_by_pan() {
    assert_eq!(SoundParams::DEFAULT.gains(), (1.0, 1.0));
    assert_eq!(SoundParams::default(), SoundParams::DEFAULT);
    let left = SoundParams {
        volume: 0.5,
        pan: -1.0,
        ..SoundParams::DEFAULT
    };
    assert_eq!(left.gains(), (0.5, 0.0));
    let right = SoundParams {
        volume: 1.0,
        pan: 0.5,
        ..SoundParams::DEFAULT
    };
    assert_eq!(right.gains(), (0.5, 1.0));
    // Out-of-range input is clamped, negative volume is silence.
    let wild = SoundParams {
        volume: -3.0,
        pan: 9.0,
        ..SoundParams::DEFAULT
    };
    assert_eq!(wild.gains(), (0.0, 0.0));
    let over = SoundParams {
        volume: 1.0,
        pan: 4.0,
        ..SoundParams::DEFAULT
    };
    assert_eq!(over.gains(), (0.0, 1.0));
}

#[test]
fn spatialize_falls_off_quadratically_and_pans_by_side() {
    let cam = Camera3 {
        eye: Vec3::new(0.0, 2.0, 0.0),
        target: Vec3::new(0.0, 2.0, 10.0),
        fov_y: 1.0,
    };
    let l = Listener::from_camera(&cam);
    assert_eq!(l.pos, cam.eye);
    assert!((l.right.x - 1.0).abs() < 1e-4 && l.right.y.abs() < 1e-4);

    // At the listener: full volume, centered.
    let here = spatialize(&l, cam.eye, 10.0, 0.8);
    assert!((here.volume - 0.8).abs() < 1e-4);
    assert_eq!(here.pan, 0.0);
    // Halfway to the radius: (1 - 0.5)^2 = 0.25 of the volume.
    let half = spatialize(&l, Vec3::new(0.0, 2.0, 5.0), 10.0, 1.0);
    assert!((half.volume - 0.25).abs() < 1e-3, "{half:?}");
    assert!(half.pan.abs() < 1e-4, "straight ahead");
    // To the right / left: pan capped at ±0.85.
    let r = spatialize(&l, Vec3::new(4.0, 2.0, 0.0), 10.0, 1.0);
    let lft = spatialize(&l, Vec3::new(-4.0, 2.0, 0.0), 10.0, 1.0);
    assert!((r.pan - 0.85).abs() < 1e-3 && (lft.pan + 0.85).abs() < 1e-3);
    assert!((r.volume - lft.volume).abs() < 1e-6);
    // At and beyond the radius: silence.
    assert_eq!(
        spatialize(&l, Vec3::new(0.0, 2.0, 10.0), 10.0, 1.0).volume,
        0.0
    );
    assert_eq!(
        spatialize(&l, Vec3::new(0.0, 50.0, 0.0), 10.0, 1.0).volume,
        0.0
    );
    // A zero radius never divides by zero.
    assert_eq!(
        spatialize(&l, Vec3::new(0.0, 2.0, 1.0), 0.0, 1.0).volume,
        0.0
    );
}

#[test]
fn ambience_keeps_the_nearest_sources_voiced_as_the_listener_walks() {
    let mut a = MockAudio::new(8);
    let mut amb: Ambience<2> = Ambience::default();
    let src = |name: &str| SoundSource {
        sound: SoundId::from_path(name),
        radius: 6.0,
        volume: 1.0,
    };
    // A river at x=0, a fire at x=10, a fountain at x=20 (all at z=0).
    let sources = [
        (0u32, Vec3::new(0.0, 0.0, 0.0), src("sounds/river")),
        (1u32, Vec3::new(10.0, 0.0, 0.0), src("sounds/fire")),
        (2u32, Vec3::new(20.0, 0.0, 0.0), src("sounds/fountain")),
    ];
    let at = |x: f32| Listener {
        pos: Vec3::new(x, 0.0, 0.0),
        right: Vec3::new(1.0, 0.0, 0.0),
    };
    assert_eq!(amb.active(), 0);

    // Next to the river: only it is in range.
    amb.update(&mut a, &at(1.0), sources);
    assert_eq!(amb.active(), 1);
    assert_eq!(a.voices.len(), 1);
    assert_eq!(a.voices[0].sound, SoundId::from_path("sounds/river"));
    assert!(a.voices[0].looped, "ambience voices loop");
    assert!(a.voices[0].params.pan < 0.0, "the river is to the left");
    let first = a.voices[0].params.volume;

    // Between river and fire: both voiced, the river now quieter.
    amb.update(&mut a, &at(4.5), sources);
    assert_eq!(amb.active(), 2);
    let river = a
        .voices
        .iter()
        .find(|v| v.sound == SoundId::from_path("sounds/river"))
        .unwrap();
    assert!(river.params.volume < first);
    assert_eq!(a.voices.len(), 2);

    // At the fire: the river leaves range and is stopped.
    amb.update(&mut a, &at(10.0), sources);
    assert_eq!(amb.active(), 1);
    assert_eq!(a.stopped.len(), 1);
    assert_eq!(a.voices[0].sound, SoundId::from_path("sounds/fire"));
    assert!((a.voices[0].params.volume - 1.0).abs() < 1e-4);

    // The fire's source despawns (not listed any more): its voice stops.
    amb.update(&mut a, &at(15.0), [sources[2]]);
    assert_eq!(amb.active(), 1);
    assert_eq!(a.voices[0].sound, SoundId::from_path("sounds/fountain"));
    assert_eq!(a.stopped.len(), 2);

    // Scene change.
    amb.stop_all(&mut a);
    assert_eq!(amb.active(), 0);
    assert!(a.voices.is_empty());
    assert_eq!(a.stopped.len(), 3);
}

#[test]
fn ambience_survives_a_mixer_without_free_voices() {
    // The mixer refuses voices: Ambience keeps its slots empty and retries.
    let mut a = MockAudio::new(0);
    let mut amb: Ambience<4> = Ambience::new();
    let src = SoundSource {
        sound: SoundId(7),
        radius: 5.0,
        volume: 1.0,
    };
    let l = Listener {
        pos: Vec3::ZERO,
        right: Vec3::new(1.0, 0.0, 0.0),
    };
    amb.update(&mut a, &l, [(0, Vec3::new(1.0, 0.0, 0.0), src)]);
    assert_eq!(amb.active(), 0);
    a.max_voices = 1;
    amb.update(&mut a, &l, [(0, Vec3::new(1.0, 0.0, 0.0), src)]);
    assert_eq!(amb.active(), 1);
    assert_eq!(a.voices.len(), 1);
}
