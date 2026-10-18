//! `cargo xtask audio inspect|report|preview|mix` through the real binary
//! (issue #53), on the engine's own assets: the self-test's PCM and codec
//! beeps, the streamed theme and the project mix.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(args: &[&str]) -> serde_json::Value {
    let o = xtask(args);
    assert!(o.status.success(), "{}", text(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn inspect_reports_codec_bytes_snr_and_memory_per_platform() {
    let doc = json(&["audio", "inspect", "music/theme", "--json"]);
    assert_eq!(doc["sound"], "music/theme");
    let platforms = doc["platforms"].as_array().unwrap();
    let by = |key: &str| platforms.iter().find(|p| p["platform"] == key).unwrap();
    for (key, codec) in [
        ("pc", "vorbis"),
        ("n64", "vadpcm"),
        ("3ds", "dspadpcm"),
        ("web", "vorbis"),
    ] {
        let p = by(key);
        assert_eq!(p["codec"], codec, "{key}");
        assert_eq!(p["auto"], true);
        assert_eq!(p["stream"], true, "music streams by default");
        assert_eq!(p["bus"], "music");
        assert!(p["ratio"].as_f64().unwrap() < 0.35, "{key}: {p}");
        assert!(p["snr_db"].as_f64().unwrap() > 25.0, "{key}: {p}");
        assert!(p["decode"].as_str().unwrap().len() > 3);
    }
    // Streams keep the encoded track (PC) or nothing (consoles) resident.
    assert_eq!(by("pc")["resident_bytes"], by("pc")["bytes"]);
    assert!(
        by("n64")["resident_bytes"].as_u64().unwrap() < 2048,
        "the cartridge holds it"
    );
    assert_eq!(by("3ds")["resident_bytes"], 0);
    // The N64 quotes its measured mixing cost; the others don't.
    let us = by("n64")["n64_us_per_frame"].as_u64().unwrap();
    assert!((100..1000).contains(&us), "{us}");
    assert!(by("pc")["n64_us_per_frame"].is_null());
    assert!(by("3ds")["playing_bytes"].as_u64().unwrap() > 0);

    // The N64's Opus is an estimate with no host decode.
    let doc = json(&[
        "audio",
        "inspect",
        "music/selftest_opus",
        "--platform",
        "n64",
        "--json",
    ]);
    let p = &doc["platforms"][0];
    assert_eq!(
        (p["codec"].as_str(), p["exact"].as_bool()),
        (Some("opus"), Some(false))
    );
    assert!(p["snr_db"].is_null());
    assert!(
        p["n64_us_per_frame"].as_u64().unwrap() > 3000,
        "Opus is the expensive one"
    );

    let o = xtask(&[
        "audio",
        "inspect",
        "sounds/selftest_pcm",
        "--platform",
        "3ds",
    ]);
    assert!(o.status.success());
    let out = text(&o.stdout);
    assert!(
        out.contains("3ds  pcm") && out.contains("SNR exact"),
        "{out}"
    );
    assert!(!out.contains(" pc "), "{out}");
}

#[test]
fn report_lists_every_sound_biggest_first_with_totals() {
    let doc = json(&["audio", "report", "--json"]);
    let sounds = doc["sounds"].as_array().unwrap();
    assert_eq!(
        sounds[0]["sound"], "music/theme",
        "the theme is the biggest"
    );
    let biggest = |s: &serde_json::Value| {
        s["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["bytes"].as_u64().unwrap())
            .max()
            .unwrap()
    };
    assert!(sounds.windows(2).all(|w| biggest(&w[0]) >= biggest(&w[1])));
    let totals = doc["totals"].as_array().unwrap();
    assert_eq!(totals.len(), 4);
    for t in totals {
        assert!(t["ratio"].as_f64().unwrap() < 0.5, "{t}");
        assert!(t["streamed"].as_u64().unwrap() >= 1);
    }
    assert!(doc["errors"].as_array().unwrap().is_empty());
}

#[test]
fn preview_writes_each_platforms_decode_as_a_wav() {
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_str().unwrap();
    let doc = json(&[
        "audio",
        "preview",
        "sounds/selftest_codec",
        "--out",
        dir,
        "--json",
    ]);
    let previews = doc["previews"].as_array().unwrap();
    assert_eq!(previews.len(), 5, "the master and four platforms");
    for p in previews {
        let file = p["file"].as_str().unwrap();
        let wav = hound::WavReader::open(file).unwrap();
        assert_eq!(wav.spec().channels, 1);
        assert!(wav.len() > 1000, "{file}");
    }
    assert!(
        std::path::Path::new(dir)
            .join("sounds_selftest_codec.n64.vadpcm.wav")
            .exists()
    );
}

#[test]
fn mix_validates_and_prints_the_project_mix() {
    let doc = json(&["audio", "mix", "--json"]);
    let music = doc["buses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["bus"] == "music")
        .unwrap();
    assert_eq!(music["volume"].as_f64().unwrap() as f32, 0.8);
    assert_eq!(doc["duck"][0]["trigger"], "voice");
    assert_eq!(doc["duck"][0]["level"].as_f64().unwrap() as f32, 0.4);
}

#[test]
fn bad_audio_invocations_fail_with_the_usage_or_a_reason() {
    let o = xtask(&["audio"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("audio inspect") && text(&o.stderr).contains("audio mix"));
    let o = xtask(&["audio", "report", "--loud"]);
    assert!(!o.status.success() && text(&o.stderr).contains("unknown argument"));
    let o = xtask(&["audio", "inspect", "sounds/nope"]);
    assert!(
        text(&o.stderr).contains("not in the manifest"),
        "{}",
        text(&o.stderr)
    );
    let o = xtask(&["audio", "inspect", "sprites/player"]);
    assert!(text(&o.stderr).contains("sounds/<name> or music/<name>"));
    let o = xtask(&["audio", "inspect", "music/theme", "--platform", "gc"]);
    assert!(!o.status.success() && text(&o.stderr).contains("--platform"));
}
