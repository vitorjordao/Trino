//! The 3DS shim's `T3AU` check (`crates/platform-3ds/shim/t3au_header.h`,
//! compiled for the host) against the pipeline's `T3au::parse` (issue
//! #121): the cases where the shim used to read a file the pipeline
//! refuses, each now refused by both, and the bake's own containers read
//! the same by both.

use trino_asset_pipeline::audio::{self as a, T3au};
use trino_fuzz::host_targets::{shim, t3au_shim};

fn header(codec: u8, flags: u8, samples: u32, data: u32, chunk: u32, ctx: u32) -> Vec<u8> {
    let mut b = b"T3AU".to_vec();
    b.extend_from_slice(&[codec, 2, flags, 1]);
    for v in [16000, samples, data, chunk, ctx] {
        b.extend_from_slice(&u32::to_le_bytes(v));
    }
    b.resize(60, 0);
    b.resize(60 + ctx as usize * 8 + data as usize, 0);
    b
}

fn both_refuse(b: &[u8], why: &str) {
    assert!(shim::parse(b).is_none(), "the shim reads {why}");
    assert!(T3au::parse(b).is_err(), "the pipeline reads {why}");
    t3au_shim(b);
}

#[test]
fn the_shim_counts_the_buses_trino_core_has() {
    assert_eq!(shim::buses(), trino_core::Bus::COUNT);
}

#[test]
fn baked_containers_read_the_same_in_c_and_rust() {
    let master = a::Master {
        rate: 16000,
        samples: (0..30000)
            .map(|i| ((i as f32) * 0.03).sin() * 0.4)
            .collect(),
    };
    for (codec, stream) in [
        (a::Codec::DspAdpcm, false),
        (a::Codec::DspAdpcm, true),
        (a::Codec::Pcm, false),
        (a::Codec::Pcm, true),
    ] {
        let settings = a::AudioSettings {
            codec,
            auto: false,
            quality: 6.0,
            sample_rate: None,
            stream,
            looped: true,
            bus: trino_core::Bus::Ambience,
        };
        let blob = a::encode(trino_asset_pipeline::Platform::N3ds, &settings, &master, 1)
            .unwrap()
            .blob
            .unwrap();
        let h = shim::parse(&blob).expect("the shim reads the bake");
        assert_eq!(h.stream != 0, stream, "{codec:?}");
        assert_eq!(h.bus as usize, trino_core::Bus::Ambience.index());
        t3au_shim(&blob);
    }
}

#[test]
fn what_the_shim_used_to_read_both_refuse() {
    // A streamed sound wasn't checked against its data: the DSP read
    // 14 336 samples per chunk out of 2 bytes.
    both_refuse(
        &header(0, 2, 30000, 2, 14336, 0),
        "a stream without its data",
    );
    both_refuse(
        &header(1, 2, 30000, 8, 14336, 3),
        "an ADPCM stream without its data",
    );
    // Any bus byte went through (the 3DS fell back to Sfx).
    let mut bus = header(0, 0, 4, 8, 0, 0);
    bus[5] = 6;
    both_refuse(&bus, "bus 6");
    // The pipeline read any channel count; the shim plays mono.
    let mut stereo = header(0, 0, 4, 8, 0, 0);
    stereo[7] = 2;
    both_refuse(&stereo, "two channels");
    // Bytes after the data, or missing ones (the stream read past the end).
    let mut long = header(0, 0, 4, 8, 0, 0);
    long.push(0);
    both_refuse(&long, "a trailing byte");
    let mut short = header(0, 2, 4, 8, 14336, 0);
    short.pop();
    both_refuse(&short, "a short stream");
    // A chunk past 2^24 samples (three wave buffers of it: the PCM byte
    // count wrapped 32 bits) and contexts past 2^20.
    both_refuse(&header(0, 2, 4, 8, 0x8000_0001, 0), "a 2^31-sample chunk");
    let mut ctx = header(1, 0, 0, 0, 0, 0);
    ctx[24..28].copy_from_slice(&(1u32 << 21).to_le_bytes());
    both_refuse(&ctx, "2^21 contexts");
    // An ADPCM chunk of partial frames, or a context count that isn't one
    // per chunk (the shim's u32 sum `samples + chunk - 1` could wrap).
    both_refuse(&header(1, 2, 28, 16, 15, 2), "a partial-frame chunk");
    both_refuse(&header(1, 2, 28672, 16384, 14336, 3), "a context too many");
    // And the edges that stay valid in both.
    for ok in [
        header(0, 0, 0, 0, 0, 0),
        header(1, 2, 0, 0, 14336, 0),
        header(1, 2, 28672, 16384, 14336, 2),
        header(0, 2, 4, 8, 14336, 0),
    ] {
        assert!(shim::parse(&ok).is_some() && T3au::parse(&ok).is_ok());
        t3au_shim(&ok);
    }
}
