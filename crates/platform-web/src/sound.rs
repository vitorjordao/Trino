//! Baked sounds for the Web (issue #53), browser-free so the host tests
//! run it: a `TSN2` blob ([`trino_core::SoundBlob`]: PCM16 or Ogg Vorbis)
//! decoded whole ([`decode`], sound effects at load), or pulled a chunk at
//! a time ([`StreamDecoder`], music as it plays) by lewton — the same
//! pure-Rust decoder the PC uses, so both play the same samples.
//!
//! lewton plays a Vorbis stream's padded last block; both paths stop at the
//! blob's frame count, so a loop's seam never hears the padding.

use std::io::Cursor;
use std::rc::Rc;

use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use trino_core::{SoundBlob, SoundCodec};

/// A sound decoded whole: mono samples (-1..1) at `rate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Decode `blob` whole (cut at its frame count).
pub fn decode(blob: &SoundBlob<'_>) -> Result<Decoded, String> {
    let mut dec = StreamDecoder::open(
        blob.codec,
        blob.sample_rate,
        blob.frames,
        blob.payload.into(),
    )?;
    let mut samples = Vec::with_capacity(blob.frames as usize);
    while dec.read(&mut samples, usize::MAX) > 0 {}
    Ok(Decoded {
        rate: dec.rate(),
        samples,
    })
}

/// PCM16 little-endian bytes as f32.
fn pcm16(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b) as f32 / 32767.0)
}

enum Source {
    Vorbis(Box<OggStreamReader<Cursor<Rc<[u8]>>>>),
    /// PCM16 bytes and the next sample.
    Pcm(usize),
}

/// A sound's samples a block at a time, from its encoded bytes (what a
/// streamed track keeps in memory): Vorbis packet by packet, PCM by slices.
pub struct StreamDecoder {
    bytes: Rc<[u8]>,
    codec: SoundCodec,
    source: Source,
    rate: u32,
    /// Samples the blob holds, and those not yet delivered.
    frames: usize,
    left: usize,
    /// Decoded samples past the last read's `max`.
    carry: Vec<f32>,
}

impl StreamDecoder {
    /// A decoder over a `TSN2` payload: `codec`, the header's rate and
    /// frame count.
    pub fn open(
        codec: SoundCodec,
        rate: u32,
        frames: u32,
        bytes: Rc<[u8]>,
    ) -> Result<Self, String> {
        let source = Self::source(codec, &bytes)?;
        let rate = match &source {
            Source::Vorbis(r) => r.ident_hdr.audio_sample_rate,
            Source::Pcm(_) => rate,
        };
        if rate == 0 {
            return Err("sound: sample rate 0".into());
        }
        Ok(StreamDecoder {
            bytes,
            codec,
            source,
            rate,
            frames: frames as usize,
            left: frames as usize,
            carry: Vec::new(),
        })
    }

    fn source(codec: SoundCodec, bytes: &Rc<[u8]>) -> Result<Source, String> {
        Ok(match codec {
            SoundCodec::Vorbis => Source::Vorbis(Box::new(
                // lewton reserves what a codebook's header says first (#121).
                trino_core::vorbis::check(bytes)
                    .map_err(|e| format!("Vorbis: {e:?}"))
                    .and_then(|()| {
                        OggStreamReader::new(Cursor::new(bytes.clone()))
                            .map_err(|e| format!("Vorbis: {e}"))
                    })?,
            )),
            SoundCodec::Pcm16 => Source::Pcm(0),
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Samples in the whole sound.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Decoded samples held between reads (memory).
    pub fn held(&self) -> usize {
        self.carry.len()
    }

    /// Append up to `max` samples to `out`; how many (0 at the end, or on
    /// a damaged packet: a broken stream ends rather than the page).
    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> usize {
        let max = max.min(self.left);
        if max == 0 {
            return 0;
        }
        let start = out.len();
        let take = self.carry.len().min(max);
        out.extend(self.carry.drain(..take));
        while out.len() - start < max {
            match &mut self.source {
                Source::Pcm(at) => {
                    let want = max - (out.len() - start);
                    let end = (*at + want * 2).min(self.bytes.len() & !1);
                    let before = out.len();
                    out.extend(pcm16(&self.bytes[*at..end]));
                    *at = end;
                    if out.len() == before {
                        break;
                    }
                }
                Source::Vorbis(reader) => {
                    let packet = match reader.read_dec_packet_generic::<InterleavedSamples<f32>>() {
                        Ok(Some(p)) => p,
                        Ok(None) | Err(_) => break,
                    };
                    let c = packet.channel_count.max(1);
                    let want = max - (out.len() - start);
                    let mut mono = packet
                        .samples
                        .chunks(c)
                        .map(|f| f.iter().sum::<f32>() / c as f32);
                    out.extend(mono.by_ref().take(want));
                    self.carry.extend(mono);
                }
            }
        }
        let n = out.len() - start;
        self.left -= n;
        if self.left == 0 {
            self.carry.clear();
        }
        n
    }

    /// Back to the first sample (a loop).
    pub fn rewind(&mut self) -> Result<(), String> {
        self.source = Self::source(self.codec, &self.bytes)?;
        self.left = self.frames;
        self.carry.clear();
        Ok(())
    }

    /// Fill `out` up to `want` samples, rewinding at the end when `looped`
    /// (seamless: the loop's first sample follows its last in the same
    /// block). False once the sound is over (not looped, or it can't
    /// rewind).
    pub fn fill(&mut self, out: &mut Vec<f32>, want: usize, looped: bool) -> bool {
        let mut stalled = false;
        while out.len() < want {
            let n = self.read(out, want - out.len());
            if n > 0 {
                stalled = false;
                continue;
            }
            // An empty sound, or one that yields nothing after a rewind,
            // must not spin.
            if !looped || stalled || self.frames == 0 || self.rewind().is_err() {
                return false;
            }
            stalled = true;
        }
        true
    }
}

/// The player's bus volumes as `localStorage` text (`trino-audio:<save>`):
/// six volumes by bus index and the mute bits
/// (`1.000,0.800,1.000,1.000,1.000,1.000;0`).
pub fn settings_to_text(s: &trino_core::BusSettings) -> String {
    let vols: Vec<String> = s.volumes.iter().map(|v| format!("{v:.3}")).collect();
    format!("{};{}", vols.join(","), s.muted.0)
}

/// Reads [`settings_to_text`]; `None` for anything else (a damaged entry
/// keeps the project's mix).
pub fn settings_from_text(text: &str) -> Option<trino_core::BusSettings> {
    let (vols, muted) = text.trim().split_once(';')?;
    let mut s = trino_core::BusSettings::default();
    let vols: Vec<&str> = vols.split(',').collect();
    if vols.len() != trino_core::Bus::COUNT {
        return None;
    }
    for (slot, v) in s.volumes.iter_mut().zip(vols) {
        *slot = v.parse::<f32>().ok()?.clamp(0.0, 1.0);
    }
    s.muted = trino_core::BusSet(muted.parse::<u8>().ok()? & 0x3F);
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(codec: SoundCodec, rate: u32, frames: u32, payload: &[u8]) -> Vec<u8> {
        let h = SoundBlob {
            codec,
            bus: trino_core::Bus::Music,
            looped: true,
            stream: true,
            sample_rate: rate,
            frames,
            payload,
        }
        .header();
        let mut b = h.to_vec();
        b.extend_from_slice(payload);
        b
    }

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn pcm_decodes_whole_and_by_blocks_with_seamless_loops() {
        let data: Vec<i16> = (0..100).map(|i| i * 100).collect();
        let b = blob(SoundCodec::Pcm16, 8000, 100, &pcm(&data));
        let parsed = SoundBlob::parse(&b).unwrap();
        let whole = decode(&parsed).unwrap();
        assert_eq!((whole.rate, whole.samples.len()), (8000, 100));
        assert!((whole.samples[99] - 9900.0 / 32767.0).abs() < 1e-6);

        let mut d =
            StreamDecoder::open(SoundCodec::Pcm16, 8000, 100, parsed.payload.into()).unwrap();
        let mut out = Vec::new();
        assert!(d.fill(&mut out, 64, true));
        assert!(d.fill(&mut out, 250, true), "a loop never ends");
        assert_eq!(out.len(), 250);
        // Sample 100 is the loop's first again, 200 too.
        assert_eq!(out[100], out[0]);
        assert_eq!(out[199], out[99]);
        assert_eq!(out[200], out[0]);
        let mut once = Vec::new();
        let mut d =
            StreamDecoder::open(SoundCodec::Pcm16, 8000, 100, parsed.payload.into()).unwrap();
        assert!(!d.fill(&mut once, 1000, false), "a one-shot ends");
        assert_eq!(once.len(), 100);

        // An empty looped sound does not spin.
        let mut e = StreamDecoder::open(SoundCodec::Pcm16, 8000, 0, Rc::from(&[][..])).unwrap();
        assert!(!e.fill(&mut Vec::new(), 10, true));
    }

    #[test]
    fn bus_settings_round_trip_through_storage_text() {
        use trino_core::{Bus, BusSettings};
        let mut s = BusSettings::default();
        s.volumes[Bus::Music.index()] = 0.25;
        s.volumes[Bus::Ui.index()] = 0.5;
        s.muted.insert(Bus::Voice);
        let text = settings_to_text(&s);
        assert_eq!(settings_from_text(&text), Some(s));
        assert_eq!(
            settings_from_text(" 1,1,1,1,1,2;0\n").unwrap().volumes[5],
            1.0
        );
        for bad in [
            "",
            "1,1,1;0",
            "1,1,1,1,1,1",
            "a,1,1,1,1,1;0",
            "1,1,1,1,1,1;x",
        ] {
            assert_eq!(settings_from_text(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn damaged_vorbis_fails_to_open_or_ends_early() {
        assert!(
            StreamDecoder::open(SoundCodec::Vorbis, 0, 10, Rc::from(&b"OggS junk"[..])).is_err()
        );
        let b = blob(SoundCodec::Vorbis, 22050, 10, b"not ogg at all");
        assert!(decode(&SoundBlob::parse(&b).unwrap()).is_err());
    }
}

/// What the bake writes: a tone through the asset pipeline's Vorbis
/// encoder, decoded here as the browser does.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod vorbis_tests {
    use super::*;
    use trino_asset_pipeline::audio as ap;

    fn tone(rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                0.4 * (t * 440.0 * std::f32::consts::TAU).sin()
                    + 0.2 * (t * 1320.0 * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    fn tsn2(ogg: &[u8], rate: u32, frames: usize) -> Vec<u8> {
        let h = SoundBlob {
            codec: SoundCodec::Vorbis,
            bus: trino_core::Bus::Music,
            looped: true,
            stream: true,
            sample_rate: rate,
            frames: frames as u32,
            payload: ogg,
        }
        .header();
        let mut b = h.to_vec();
        b.extend_from_slice(ogg);
        b
    }

    #[test]
    fn vorbis_decodes_like_the_pc_whole_and_in_stream_chunks() {
        let pcm = tone(22_050, 2.0);
        let ogg = ap::vorbis::encode(&pcm, 22_050, 6.0, 1).unwrap();
        let blob = tsn2(&ogg, 22_050, pcm.len());
        let b = SoundBlob::parse(&blob).unwrap();
        let whole = decode(&b).unwrap();
        assert_eq!((whole.rate, whole.samples.len()), (22_050, pcm.len()));
        // The PC's decode (the bake's preview) is the same samples.
        let (_, pc) = ap::vorbis::decode(&ogg).unwrap();
        assert_eq!(whole.samples, pc);
        let snr = ap::snr_db(&pcm, &whole.samples);
        assert!(snr >= 30.0, "q6: {snr:.1} dB");

        // Streamed in 8192-sample chunks: the same samples, and a loop's
        // seam goes straight back to the first sample (no padding).
        let mut d =
            StreamDecoder::open(b.codec, b.sample_rate, b.frames, b.payload.into()).unwrap();
        let mut streamed = Vec::new();
        let mut chunk = Vec::new();
        while streamed.len() < pcm.len() + 8192 {
            chunk.clear();
            assert!(d.fill(&mut chunk, 8192, true));
            assert!(d.held() < 4096, "carry stays under a packet");
            streamed.extend_from_slice(&chunk);
        }
        assert_eq!(&streamed[..pcm.len()], &whole.samples[..]);
        assert_eq!(&streamed[pcm.len()..pcm.len() + 100], &whole.samples[..100]);
    }
}
