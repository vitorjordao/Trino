//! `cargo xtask audio inspect <sounds/x|music/x> [--platform pc|n64|3ds|web]
//! [--json]` — what each platform keeps of a sound: codec and why, rate,
//! bytes on the medium (ROM, RomFS, asset file, download) and against
//! PCM16, SNR of what it plays against the master, RAM resident and while
//! playing, who decodes it (`trino_asset_pipeline::audio::inspect`, the
//! editor's audio inspector shows the same).
//!
//! `cargo xtask audio report [--platform ..] [--json]` — every sound and
//! track of the project, biggest first, with each platform's totals.
//!
//! `cargo xtask audio preview <sounds/x|music/x> [--platform ..] [--out
//! <dir>] [--json]` — the decoded console version as a WAV per platform
//! (and the master), for listening tests: VADPCM as libdragon's decoder
//! plays it, DSP-ADPCM as the 3DS DSP does, Vorbis as lewton does. The
//! N64's Opus only decodes on the console.
//!
//! `cargo xtask audio mix [--json]` — validate `assets/audio.toml` (the
//! editor's Mixer panel file) and print the mix it bakes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_asset_pipeline::audio::{self, AudioReport, MixFile};
use trino_asset_pipeline::{Platform, decode_master};

const USAGE: &str = "xtask: usage:
  cargo xtask audio inspect <sounds/x|music/x> [--platform pc|n64|3ds|web] [--json]
  cargo xtask audio report [--platform pc|n64|3ds|web] [--json]
  cargo xtask audio preview <sounds/x|music/x> [--platform ..] [--out <dir>] [--json]
  cargo xtask audio mix [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let assets = root.join("assets");
    let result = match args {
        ["inspect", logical, rest @ ..] => {
            options(rest).and_then(|o| inspect(&assets, logical, &o))
        }
        ["report", rest @ ..] => options(rest).and_then(|o| report(&assets, &o)),
        ["preview", logical, rest @ ..] => options(rest).and_then(|o| {
            let out = o
                .out
                .clone()
                .unwrap_or_else(|| root.join("target/audio-preview"));
            preview(&assets, logical, &o, &out)
        }),
        ["mix", rest @ ..] => options(rest).and_then(|o| mix(&assets, o.json)),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    platforms: Vec<Platform>,
    json: bool,
    out: Option<PathBuf>,
}

fn options(rest: &[&str]) -> Result<Options, String> {
    let mut o = Options {
        platforms: Platform::ALL.to_vec(),
        json: false,
        out: None,
    };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--json" => o.json = true,
            "--platform" => match it.next().and_then(|p| Platform::parse(p)) {
                Some(p) => o.platforms = vec![p],
                None => return Err("xtask: --platform needs pc, n64, 3ds or web".into()),
            },
            "--out" => match it.next() {
                Some(d) => o.out = Some(PathBuf::from(d)),
                None => return Err("xtask: --out needs a directory".into()),
            },
            other => return Err(format!("xtask: unknown argument `{other}`\n{USAGE}")),
        }
    }
    Ok(o)
}

fn to_json(r: &AudioReport) -> serde_json::Value {
    let e = &r.encoded;
    serde_json::json!({
        "platform": e.platform.key(),
        "codec": e.settings.codec.label(),
        "auto": e.settings.auto,
        "why": r.why,
        "rate": e.rate,
        "frames": e.frames,
        "seconds": e.frames as f64 / e.rate.max(1) as f64,
        "bytes": r.bytes,
        "exact": e.exact,
        "pcm16_bytes": e.pcm16_bytes(),
        "ratio": r.ratio(),
        "snr_db": r.snr_db,
        "stream": e.settings.stream,
        "loop": e.settings.looped,
        "bus": e.settings.bus.label(),
        "resident_bytes": r.resident_bytes,
        "playing_bytes": r.playing_bytes,
        "decode": r.decode,
        "n64_us_per_frame": r.n64_us_per_frame,
    })
}

fn snr(r: &AudioReport) -> String {
    match r.snr_db {
        Some(db) if db > 90.0 => "exact".into(),
        Some(db) => format!("{db:.1} dB"),
        None => "n/a (console-only decode)".into(),
    }
}

fn line(r: &AudioReport) -> String {
    let e = &r.encoded;
    let approx = if e.exact { "" } else { "~" };
    let playing = if r.playing_bytes > 0 {
        format!(" (+{} B playing)", r.playing_bytes)
    } else {
        String::new()
    };
    format!(
        "  {:<4} {:<8} {:>5} Hz {:>6.2} s  {approx}{:>8} B  {:>5.1}% of PCM16  SNR {:<10} {}  bus {:<8} RAM {} B{playing}",
        e.platform.key(),
        e.settings.codec.label(),
        e.rate,
        e.frames as f64 / e.rate.max(1) as f64,
        r.bytes,
        r.ratio() * 100.0,
        snr(r),
        if e.settings.stream {
            "stream"
        } else {
            "loaded"
        },
        e.settings.bus.label(),
        r.resident_bytes,
    )
}

fn inspect(assets: &Path, logical: &str, o: &Options) -> Result<(), String> {
    let reports = audio::inspect(assets, logical, &o.platforms)?;
    if o.json {
        let list: Vec<_> = reports.iter().map(to_json).collect();
        let doc = serde_json::json!({ "sound": logical, "platforms": list });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
        return Ok(());
    }
    println!("{logical}");
    for r in &reports {
        println!("{}", line(r));
        let cost = match r.n64_us_per_frame {
            Some(us) => format!(" (~{us} µs of a 60 Hz frame per voice)"),
            None => String::new(),
        };
        println!("       why: {}; decode: {}{cost}", r.why, r.decode);
    }
    Ok(())
}

/// Per-platform sums for `report`.
#[derive(Default)]
struct Totals {
    bytes: usize,
    pcm16: usize,
    resident: usize,
    streams: usize,
    estimated: bool,
}

fn report(assets: &Path, o: &Options) -> Result<(), String> {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (logical, r) in audio::report_project(assets, &o.platforms)? {
        match r {
            Ok(r) => rows.push((logical, r)),
            Err(e) => failed.push((logical, e)),
        }
    }
    let biggest = |r: &Vec<AudioReport>| r.iter().map(|r| r.bytes).max().unwrap_or(0);
    rows.sort_by(|a, b| biggest(&b.1).cmp(&biggest(&a.1)).then(a.0.cmp(&b.0)));
    let mut totals: Vec<(Platform, Totals)> = o
        .platforms
        .iter()
        .map(|&p| (p, Totals::default()))
        .collect();
    for (_, reports) in &rows {
        for r in reports {
            let t = &mut totals
                .iter_mut()
                .find(|(p, _)| *p == r.encoded.platform)
                .unwrap()
                .1;
            t.bytes += r.bytes;
            t.pcm16 += r.encoded.pcm16_bytes();
            t.resident += r.resident_bytes;
            t.streams += r.encoded.settings.stream as usize;
            t.estimated |= !r.encoded.exact;
        }
    }
    if o.json {
        let sounds: Vec<_> = rows
            .iter()
            .map(|(l, r)| {
                serde_json::json!({ "sound": l, "platforms": r.iter().map(to_json).collect::<Vec<_>>() })
            })
            .collect();
        let totals: Vec<_> = totals
            .iter()
            .map(|(p, t)| {
                serde_json::json!({
                    "platform": p.key(),
                    "bytes": t.bytes,
                    "pcm16_bytes": t.pcm16,
                    "ratio": t.bytes as f64 / t.pcm16.max(1) as f64,
                    "resident_bytes": t.resident,
                    "streamed": t.streams,
                    "estimated": t.estimated,
                })
            })
            .collect();
        let errors: Vec<_> = failed
            .iter()
            .map(|(l, e)| serde_json::json!({ "sound": l, "error": e }))
            .collect();
        let doc = serde_json::json!({ "sounds": sounds, "totals": totals, "errors": errors });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        for (logical, reports) in &rows {
            println!("{logical}");
            for r in reports {
                println!("{}", line(r));
            }
        }
        println!("totals:");
        for (p, t) in &totals {
            println!(
                "  {:<4} {}{} B on the medium vs {} B PCM16 ({:.1}%), {} B RAM resident, {} streamed",
                p.key(),
                if t.estimated { "~" } else { "" },
                t.bytes,
                t.pcm16,
                t.bytes as f64 * 100.0 / t.pcm16.max(1) as f64,
                t.resident,
                t.streams
            );
        }
        for (l, e) in &failed {
            println!("FAILED {l}: {e}");
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} sound(s) would fail the bake", failed.len()))
    }
}

fn preview(assets: &Path, logical: &str, o: &Options, out: &Path) -> Result<(), String> {
    let reports = audio::inspect(assets, logical, &o.platforms)?;
    let name = logical.replace('/', "_");
    let mut written = Vec::new();
    // The master too, for A/B listening.
    let manifest = trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml"))?;
    let file = match logical.split_once('/') {
        Some(("sounds", n)) => manifest.sounds.get(n),
        Some(("music", n)) => manifest.music.get(n),
        _ => None,
    }
    .map(|d| d.file.clone())
    .ok_or_else(|| format!("`{logical}` is not in the manifest"))?;
    let source = trino_asset_pipeline::resolve::resolve_source(assets, Platform::Pc, &file)?;
    let master = decode_master(&source)?;
    let master_path = out.join(format!("{name}.master.wav"));
    audio::write_wav(&master_path, master.rate, &master.samples)?;
    written.push(
        serde_json::json!({ "platform": "master", "file": master_path, "rate": master.rate }),
    );
    for r in &reports {
        let e = &r.encoded;
        let key = e.platform.key();
        match &e.decoded {
            Some(samples) => {
                let path = out.join(format!("{name}.{key}.{}.wav", e.settings.codec.label()));
                audio::write_wav(&path, e.rate, samples)?;
                written.push(serde_json::json!({
                    "platform": key,
                    "codec": e.settings.codec.label(),
                    "file": path,
                    "rate": e.rate,
                    "bytes": r.bytes,
                    "snr_db": r.snr_db,
                }));
            }
            None => written.push(serde_json::json!({
                "platform": key,
                "codec": e.settings.codec.label(),
                "file": null,
                "note": "decodes only on the console (libdragon Opus); listen in ares",
            })),
        }
    }
    if o.json {
        let doc = serde_json::json!({ "sound": logical, "previews": written });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        for w in &written {
            match w["file"].as_str() {
                Some(f) => println!("{:<6} {f}", w["platform"].as_str().unwrap()),
                None => println!(
                    "{:<6} (none: {})",
                    w["platform"].as_str().unwrap(),
                    w["note"].as_str().unwrap()
                ),
            }
        }
    }
    Ok(())
}

fn mix(assets: &Path, json: bool) -> Result<(), String> {
    let path = assets.join(MixFile::FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            // No file: every bus at 1, no duck rules (what the runtime does).
            MixFile {
                version: 1,
                ..MixFile::default()
            }
            .to_toml()
        }
    };
    let file = MixFile::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg = file.config()?;
    let buses: Vec<_> = trino_core::Bus::ALL
        .iter()
        .map(|b| serde_json::json!({ "bus": b.label(), "volume": cfg.volumes[b.index()] }))
        .collect();
    let ducks: Vec<_> = cfg
        .ducks
        .iter()
        .flatten()
        .map(|d| {
            serde_json::json!({
                "trigger": d.trigger.label(),
                "target": d.target.label(),
                "level": d.level,
                "attack_ms": d.attack_ms,
                "release_ms": d.release_ms,
            })
        })
        .collect();
    if json {
        let doc = serde_json::json!({ "file": path, "buses": buses, "duck": ducks });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        println!("{}", path.display());
        for b in trino_core::Bus::ALL {
            println!("  {:<8} {:.2}", b.label(), cfg.volumes[b.index()]);
        }
        for d in cfg.ducks.iter().flatten() {
            println!(
                "  duck: {} pulls {} to {:.2} ({} ms down, {} ms back)",
                d.trigger.label(),
                d.target.label(),
                d.level,
                d.attack_ms,
                d.release_ms
            );
        }
    }
    Ok(())
}
