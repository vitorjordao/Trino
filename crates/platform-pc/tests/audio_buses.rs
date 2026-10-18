//! Buses, ducking, the project's mix, persistence and seamless streamed
//! loops on the PC backend (issue #53), rendered offline so every sample is
//! checked without an audio device.

use trino_core::{
    Audio, Bus, BusSettings, DuckRule, MixerConfig, MusicId, SoundBlob, SoundCodec, SoundId,
    SoundParams,
};
use trino_platform_pc::PcAudio;
use trino_platform_pc::audio::{settings_from_text, settings_to_text};

const RATE: u32 = 1000;

/// Left channel after rendering `frames` frames.
fn render(audio: &mut PcAudio, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames * 2];
    audio.render(&mut out);
    out.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
}

fn constant(value: f32) -> Vec<f32> {
    vec![value; RATE as usize * 30]
}

#[test]
fn bus_volumes_and_mutes_scale_only_their_bus() {
    let mut audio = PcAudio::new_offline(RATE);
    audio.upload_sound(SoundId(1), constant(0.4));
    audio.upload_music(MusicId(2), constant(0.2));
    audio.play_sound(SoundId(1));
    audio.play_music(MusicId(2), true);
    let both = render(&mut audio, 10)[9];
    assert!((both - 0.6).abs() < 1e-5, "{both}");

    audio.set_bus_volume(Bus::Sfx, 0.5);
    let half_sfx = render(&mut audio, 10)[9];
    assert!((half_sfx - (0.2 + 0.2)).abs() < 1e-5, "{half_sfx}");

    // The old calls are the buses.
    audio.set_music_volume(0.0);
    assert_eq!(audio.bus_volume(Bus::Music), 0.0);
    let no_music = render(&mut audio, 10)[9];
    assert!((no_music - 0.2).abs() < 1e-5, "{no_music}");
    audio.set_music_volume(1.0);
    audio.set_master_volume(0.5);
    let master = render(&mut audio, 10)[9];
    assert!((master - 0.5 * 0.4).abs() < 1e-5, "{master}");

    // Mutes keep the volume underneath.
    audio.set_bus_muted(Bus::Music, true);
    assert!(audio.bus_muted(Bus::Music));
    let muted = render(&mut audio, 10)[9];
    assert!((muted - 0.5 * 0.2).abs() < 1e-5, "{muted}");
    audio.set_bus_muted(Bus::Music, false);
    assert_eq!(audio.bus_volume(Bus::Music), 1.0);

    // A sound sent to another bus follows that bus.
    audio.stop_music();
    audio.set_bus_volume(Bus::Ui, 0.0);
    audio.play_sound_with(SoundId(1), &SoundParams::DEFAULT.on(Bus::Ui));
    let on_ui = render(&mut audio, 10)[9];
    assert!(
        (on_ui - 0.5 * 0.5 * 0.4).abs() < 1e-5,
        "only the sfx one: {on_ui}"
    );
}

/// The issue's example rule: voice ducks the music to 0.4 over 150 ms and
/// lets it back over 300 ms — linear ramps timed by the rendered audio.
#[test]
fn a_voice_ducks_the_music_and_lets_it_back() {
    let mut audio = PcAudio::new_offline(RATE);
    audio.upload_sound(SoundId(1), vec![0.0; RATE as usize / 2]); // 0.5 s of "speech"
    audio.upload_music(MusicId(2), constant(0.5));
    audio.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300));
    audio.play_music(MusicId(2), true);
    assert_eq!(render(&mut audio, 50)[49], 0.5, "nothing on the voice bus");

    audio.play_sound_with(SoundId(1), &SoundParams::DEFAULT.on(Bus::Voice));
    // Ramps step once per rendered buffer: 15 buffers of 10 ms.
    let mut levels = Vec::new();
    for _ in 0..20 {
        render(&mut audio, 10);
        levels.push(audio.bus_duck(Bus::Music));
    }
    assert!((levels[6] - 0.72).abs() < 0.05, "halfway down: {levels:?}");
    assert!(
        (levels[15] - 0.4).abs() < 1e-4,
        "down after 150 ms: {levels:?}"
    );
    let ducked = render(&mut audio, 10)[9];
    assert!((ducked - 0.5 * 0.4).abs() < 1e-4, "{ducked}");

    // The 0.5 s voice ends; the music comes back over 300 ms.
    render(&mut audio, 300);
    assert_eq!(audio.audio_stats().voices, 1, "the voice is over");
    for _ in 0..15 {
        render(&mut audio, 10);
    }
    let back = audio.bus_duck(Bus::Music);
    assert!((back - 0.7).abs() < 0.05, "halfway back: {back}");
    for _ in 0..20 {
        render(&mut audio, 10);
    }
    assert_eq!(audio.bus_duck(Bus::Music), 1.0);

    // A level of 1 removes the rule; clear_ducks removes them all.
    audio.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 1.0, 0, 0));
    audio.play_sound_with(SoundId(1), &SoundParams::DEFAULT.on(Bus::Voice));
    render(&mut audio, 100);
    assert_eq!(audio.bus_duck(Bus::Music), 1.0);
    audio.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.0, 0, 0));
    render(&mut audio, 10);
    assert_eq!(audio.bus_duck(Bus::Music), 0.0, "instant duck to silence");
    audio.clear_ducks();
    render(&mut audio, 10);
    assert_eq!(audio.bus_duck(Bus::Music), 1.0);
}

/// `assets/audio.toml` baked (`TMIX`) sets the starting mix; the player's
/// saved volumes (`<save>.audio`) apply over it and survive a round trip.
#[test]
fn the_project_mix_and_the_players_settings() {
    let mut audio = PcAudio::new_offline(RATE);
    let mut cfg = MixerConfig::default();
    cfg.volumes[Bus::Music.index()] = 0.8;
    cfg.ducks[0] = Some(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300));
    let mut tmix = vec![0u8; cfg.encoded_len()];
    cfg.encode(&mut tmix);
    let parsed = MixerConfig::parse(&tmix).unwrap();
    parsed.apply(&mut audio);
    assert!((audio.bus_volume(Bus::Music) - 0.8).abs() < 1e-3);

    // The player turns the music down and mutes the UI.
    audio.set_bus_volume(Bus::Music, 0.25);
    audio.set_bus_muted(Bus::Ui, true);
    let text = settings_to_text(&audio.bus_settings());
    assert!(
        text.contains("music 0.25") && text.contains("ui 1.00 muted"),
        "{text}"
    );

    let mut fresh = PcAudio::new_offline(RATE);
    parsed.apply(&mut fresh);
    settings_from_text(&text).apply(&mut fresh);
    assert_eq!(fresh.bus_settings(), audio.bus_settings());
    // Hand-edited garbage keeps the defaults; volumes clamp.
    let odd = settings_from_text("# x\nmusic loud\nbogus 0.5\nsfx 7\nvoice 0.5 muted extra\n");
    assert_eq!(odd.volumes[Bus::Music.index()], 1.0);
    assert_eq!(odd.volumes[Bus::Sfx.index()], 1.0);
    assert_eq!(odd.volumes[Bus::Voice.index()], 0.5);
    assert!(odd.muted.contains(Bus::Voice));
    // Consoles pack the same settings into 4 bytes of a save.
    let packed = BusSettings::from_bytes(audio.bus_settings().to_bytes());
    assert!((packed.volumes[Bus::Music.index()] - 0.25).abs() < 0.04);
    assert!(packed.muted.contains(Bus::Ui));
}

/// A streamed, looped Vorbis track repeats sample for sample: the seam
/// adds no gap (lewton's padded last block is cut at the baked length).
#[test]
fn streamed_vorbis_loops_without_a_gap() {
    let rate = 8000u32;
    let len = 3001usize; // not a multiple of any block size
    let pcm: Vec<f32> = (0..len)
        .map(|i| 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin())
        .collect();
    let ogg = trino_asset_pipeline::audio::vorbis::encode(&pcm, rate, 6.0, 1).unwrap();
    let blob = |stream: bool| {
        let header = SoundBlob {
            codec: SoundCodec::Vorbis,
            bus: Bus::Music,
            looped: true,
            stream,
            sample_rate: rate,
            frames: len as u32,
            payload: &ogg,
        }
        .header();
        [header.as_slice(), &ogg].concat()
    };
    for stream in [true, false] {
        let mut audio = PcAudio::new_offline(rate);
        audio.upload_music_blob(MusicId(1), &blob(stream)).unwrap();
        audio.play_music(MusicId(1), true);
        // Device-sized buffers: the ring holds half a second.
        let out: Vec<f32> = (0..len * 3 / 250 + 1)
            .flat_map(|_| render(&mut audio, 250))
            .collect();
        assert_eq!(audio.audio_stats().streams, stream as u16);
        for i in 0..len {
            assert!(
                (out[i] - out[i + len]).abs() < 1e-6 && (out[i] - out[i + 2 * len]).abs() < 1e-6,
                "stream={stream}: sample {i} differs across the seam"
            );
        }
        assert_eq!(audio.audio_stats().underruns, 0);
    }
}
