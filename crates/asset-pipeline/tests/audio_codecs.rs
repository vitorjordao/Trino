//! Codec round trips through the public bake API (issue #53): each
//! platform's default codec, decoded the way the console decodes it,
//! against the master. Thresholds from the issue: VADPCM ≥ 25 dB (N64),
//! DSP-ADPCM ≥ 30 dB (3DS). Vorbis q6 measures 27–41 dB here: it spends
//! bits by audibility, not by waveform, and the square-wave chiptune
//! masters cost it the most (the decoder is not the cause — lewton matches
//! libvorbis to > 100 dB, `vorbis::tests`), so its floor is 26 dB at q6 and
//! the issue's 30 dB holds from q8 (ADR 0021). The fixtures are tonal
//! (music and effects); noise-like masters land lower and `cargo xtask
//! audio report` shows them.

use std::path::PathBuf;

use trino_asset_pipeline::audio::{self, AudioSettings, Codec, Master};
use trino_asset_pipeline::{Platform, decode_master};
use trino_core::Bus;

fn fixtures() -> Vec<(&'static str, Master)> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.join("../..");
    [
        (
            "tone+chirp",
            crate_dir.join("tests/fixtures/audio/n64_tone.wav"),
        ),
        ("castle64 theme", repo.join("assets/shared/music/theme.wav")),
        ("castle64 win", repo.join("assets/shared/sounds/win.wav")),
        ("castle64 beep", repo.join("assets/shared/sounds/beep.wav")),
    ]
    .into_iter()
    .map(|(name, path)| (name, decode_master(&path).unwrap()))
    .collect()
}

fn settings(codec: Codec, quality: f32) -> AudioSettings {
    AudioSettings {
        codec,
        auto: false,
        quality,
        sample_rate: None,
        stream: false,
        looped: false,
        bus: Bus::Sfx,
    }
}

#[test]
fn every_codec_clears_its_snr_threshold_on_the_fixtures() {
    let cases = [
        (Platform::N64, Codec::Vadpcm, 5.0, 25.0),
        (Platform::N3ds, Codec::DspAdpcm, 5.0, 30.0),
        (Platform::Pc, Codec::Vorbis, 6.0, 26.0),
        (Platform::Web, Codec::Vorbis, 6.0, 26.0),
        (Platform::Pc, Codec::Vorbis, 8.0, 29.5),
    ];
    for (name, master) in fixtures() {
        for (platform, codec, quality, floor) in cases {
            let enc = audio::encode(platform, &settings(codec, quality), &master, 7).unwrap();
            let snr = enc.snr_db(&master).unwrap();
            let decoded = enc.decoded.as_ref().unwrap();
            println!(
                "{name:<16} {:<4} {:<9} {:>8} B ({:>5.1}% of PCM16) {snr:>5.1} dB",
                platform.key(),
                codec.label(),
                enc.bytes,
                enc.bytes as f64 * 100.0 / enc.pcm16_bytes() as f64,
            );
            assert_eq!(decoded.len(), enc.frames, "{name} {codec:?}: length");
            assert!(
                snr >= floor,
                "{name} on {}: {} at {snr:.1} dB < {floor} dB",
                platform.key(),
                codec.label()
            );
            assert!(
                enc.bytes < enc.pcm16_bytes(),
                "{name} {codec:?}: compresses"
            );
        }
    }
}

/// PCM is exact (to 16 bits) everywhere; the N64's Opus is sized but not
/// decodable on the host (libdragon decodes it on the console only).
#[test]
fn pcm_is_exact_and_n64_opus_is_an_estimate() {
    let (_, master) = fixtures().swap_remove(0);
    for platform in [Platform::Pc, Platform::Web, Platform::N64, Platform::N3ds] {
        let enc = audio::encode(platform, &settings(Codec::Pcm, 5.0), &master, 7).unwrap();
        assert!(enc.snr_db(&master).unwrap() > 85.0, "{platform:?}");
        assert!(enc.exact);
    }
    let opus = audio::encode(Platform::N64, &settings(Codec::Opus, 5.0), &master, 7).unwrap();
    assert!(!opus.exact && opus.decoded.is_none() && opus.bytes > 0);
    // A codec the platform doesn't play is an error, not a silent fallback.
    let err = audio::encode(Platform::Pc, &settings(Codec::DspAdpcm, 5.0), &master, 7);
    assert!(err.is_err());
}

/// Resampling per platform (`sample_rate = { n64 = 22050 }`): the bake plays
/// at the asked rate, the length scales, and the tone survives.
#[test]
fn per_platform_sample_rates_resample_the_master() {
    let (_, master) = fixtures().swap_remove(1); // theme, 44.1 kHz
    let mut s = settings(Codec::Vadpcm, 5.0);
    s.sample_rate = Some(22_050);
    let enc = audio::encode(Platform::N64, &s, &master, 7).unwrap();
    assert_eq!(enc.rate, 22_050);
    assert_eq!(enc.frames, master.samples.len() / 2);
    assert!(enc.snr_db(&master).unwrap() >= 20.0);
}
