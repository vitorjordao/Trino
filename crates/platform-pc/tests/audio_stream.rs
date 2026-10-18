//! Streaming keeps RAM flat (issue #53): a 10-minute Vorbis track plays to
//! its end through the PC backend while a counting allocator watches the
//! heap. Only the encoded bytes stay resident; the decoder, resampler and
//! ring hold a fixed amount however long the track. Alone in its binary so
//! no other test's allocations reach the counter.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use trino_asset_pipeline::Platform;
use trino_asset_pipeline::audio::{AudioSettings, Codec, Master};
use trino_core::{Audio, Bus, MusicId};
use trino_platform_pc::PcAudio;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const RATE: u32 = 16_000;
const OUT_RATE: u32 = 48_000;
const MINUTES: u32 = 10;

/// A 10-minute melody (a note every quarter second, two partials): enough
/// change that the encoder can't collapse it to nothing.
fn melody() -> Vec<f32> {
    let n = (RATE * 60 * MINUTES) as usize;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let note = (i / (RATE as usize / 4)) % 12;
            let f = 220.0 * 2f32.powf(note as f32 / 12.0);
            phase = (phase + f * std::f32::consts::TAU / RATE as f32) % std::f32::consts::TAU;
            0.3 * phase.sin() + 0.1 * (phase * 3.0).sin()
        })
        .collect()
}

#[test]
fn a_ten_minute_stream_plays_with_bounded_ram() {
    let master = Master {
        rate: RATE,
        samples: melody(),
    };
    let settings = AudioSettings {
        codec: Codec::Vorbis,
        auto: false,
        quality: 2.0,
        sample_rate: None,
        stream: true,
        looped: false,
        bus: Bus::Music,
    };
    let enc = trino_asset_pipeline::audio::encode(Platform::Pc, &settings, &master, 1).unwrap();
    let blob = enc.blob.unwrap();
    drop(master);
    let decoded_whole = (RATE * 60 * MINUTES) as usize * OUT_RATE as usize / RATE as usize * 4;

    let mut audio = PcAudio::new_offline(OUT_RATE);
    audio.upload_music_blob(MusicId(1), &blob).unwrap();
    let resident = audio.audio_stats().resident_bytes as usize;
    assert!(
        resident <= blob.len() && resident * 20 < decoded_whole,
        "only the encoded track is resident: {resident} B"
    );
    drop(blob);

    let mut out = vec![0.0f32; 1024 * 2];
    // From before the stream starts: its ring and decoder count.
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    audio.play_music(MusicId(1), false);
    let mut frames = 0u64;
    let mut energy = 0.0f64;
    let mut stream_bytes = 0;
    while audio.audio_stats().voices > 0 {
        audio.render(&mut out);
        frames += 1024;
        energy += out.iter().map(|&s| s as f64 * s as f64).sum::<f64>();
        stream_bytes = stream_bytes.max(audio.audio_stats().stream_bytes);
        assert!(
            frames < OUT_RATE as u64 * 60 * (MINUTES as u64 + 1),
            "never ends"
        );
    }
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let stats = audio.audio_stats();
    let want = (RATE * 60 * MINUTES) as u64 * OUT_RATE as u64 / RATE as u64;
    println!(
        "TRINO_STREAM_RAM: minutes={MINUTES} encoded={resident} B decoded_whole={decoded_whole} B \
         peak_while_playing={peak} B ring={stream_bytes} B frames={} underruns={}",
        stats.streamed_frames, stats.underruns
    );
    assert!(
        (stats.streamed_frames as u64).abs_diff(want) < 4,
        "every sample played: {} of {want}",
        stats.streamed_frames
    );
    assert_eq!(
        stats.underruns, 0,
        "offline, the decoder never falls behind"
    );
    assert!(
        peak < 1 << 20,
        "playing 10 minutes grew the heap by {peak} B (decoded whole: {decoded_whole} B)"
    );
    assert!(
        (energy / (frames * 2) as f64).sqrt() > 0.05,
        "it was audible"
    );
    assert_eq!(stats.streams, 0, "the finished stream let its ring go");
}
