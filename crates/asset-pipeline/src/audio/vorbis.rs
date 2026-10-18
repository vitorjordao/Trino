//! Ogg Vorbis for the PC and the Web: libvorbis (aoTuV) encodes in the bake,
//! lewton decodes — the same pure-Rust decoder the PC runtime and the
//! browser's wasm use, so a preview here is what they play.

use std::io::Cursor;
use std::num::{NonZeroU8, NonZeroU32};

use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

/// Encode mono `samples` (-1..1) at `rate` with `quality` 0..=10 (`oggenc
/// -q`). `serial` names the Ogg stream: pass something stable (the asset
/// id) so a bake is byte-for-byte reproducible.
pub fn encode(samples: &[f32], rate: u32, quality: f32, serial: i32) -> Result<Vec<u8>, String> {
    let rate = NonZeroU32::new(rate).ok_or("Vorbis: sample rate 0")?;
    let mut out = Vec::new();
    {
        let mut enc = VorbisEncoderBuilder::new_with_serial(rate, NonZeroU8::MIN, &mut out, serial)
            .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
                target_quality: (quality / 10.0).clamp(-0.1, 1.0),
            })
            .build()
            .map_err(|e| format!("Vorbis encoder: {e}"))?;
        // A block of 1024 is libvorbis's own suggestion.
        for block in samples.chunks(1024) {
            enc.encode_audio_block([block])
                .map_err(|e| format!("Vorbis encode: {e}"))?;
        }
        enc.finish().map_err(|e| format!("Vorbis encode: {e}"))?;
    }
    Ok(out)
}

/// Decode an Ogg Vorbis file to mono (stereo is averaged): `(rate, samples)`,
/// cut at the last page's granule position (lewton plays the whole last
/// block; the encoder padded it).
pub fn decode(bytes: &[u8]) -> Result<(u32, Vec<f32>), String> {
    let mut reader =
        OggStreamReader::new(Cursor::new(bytes)).map_err(|e| format!("Vorbis decode: {e}"))?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = reader.ident_hdr.audio_channels as usize;
    let mut out = Vec::new();
    while let Some(packet) = reader
        .read_dec_packet_generic::<InterleavedSamples<f32>>()
        .map_err(|e| format!("Vorbis decode: {e}"))?
    {
        if channels == 1 {
            out.extend_from_slice(&packet.samples);
        } else {
            out.extend(
                packet
                    .samples
                    .chunks(channels)
                    .map(|f| f.iter().sum::<f32>() / channels as f32),
            );
        }
    }
    if let Some(end) = last_granule(bytes) {
        out.truncate(end as usize);
    }
    Ok((rate, out))
}

/// The granule position (samples) of the last Ogg page: where the stream
/// really ends.
pub fn last_granule(bytes: &[u8]) -> Option<u64> {
    let at = bytes.windows(4).rposition(|w| w == b"OggS")?;
    let g = u64::from_le_bytes(bytes.get(at + 6..at + 14)?.try_into().ok()?);
    (g != u64::MAX).then_some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                0.4 * (t * 440.0 * std::f32::consts::TAU).sin()
                    + 0.2 * (t * 1320.0 * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    #[test]
    fn round_trip_keeps_length_and_is_reproducible() {
        let pcm = tone(22050, 1.3);
        let a = encode(&pcm, 22050, 6.0, 7).unwrap();
        let b = encode(&pcm, 22050, 6.0, 7).unwrap();
        assert_eq!(a, b, "same input, same serial: same bytes");
        assert!(
            a.len() < pcm.len() * 2 / 4,
            "smaller than a quarter of PCM16"
        );
        let (rate, back) = decode(&a).unwrap();
        assert_eq!(rate, 22050);
        assert_eq!(back.len(), pcm.len(), "the last granule trims the tail");
        assert_eq!(last_granule(&a), Some(pcm.len() as u64));
        let snr = crate::audio::snr_db(&pcm, &back);
        assert!(snr >= 30.0, "q6 tone: {snr:.1} dB");
        // A cut file fails or decodes short; it never reads past its end.
        if let Ok((_, s)) = decode(&a[..a.len() / 3]) {
            assert!(s.len() < pcm.len());
        }
        assert!(decode(b"OggS but not really").is_err());
    }

    /// The runtime decoder (lewton, on the PC and in the browser's wasm)
    /// plays what libvorbis itself decodes: what the bake previews is what
    /// players hear, and codec noise is the encoder's, never the decoder's.
    #[test]
    fn lewton_matches_libvorbis() {
        let pcm = tone(44_100, 0.8);
        let ogg = encode(&pcm, 44_100, 6.0, 3).unwrap();
        let (_, ours) = decode(&ogg).unwrap();
        let mut dec = vorbis_rs::VorbisDecoder::new(Cursor::new(ogg)).unwrap();
        let mut reference = Vec::new();
        while let Some(block) = dec.decode_audio_block().unwrap() {
            reference.extend_from_slice(block.samples()[0]);
        }
        reference.truncate(ours.len());
        let agree = crate::audio::snr_db(&reference, &ours);
        assert!(agree > 100.0, "lewton vs libvorbis: {agree:.1} dB");
    }
}
