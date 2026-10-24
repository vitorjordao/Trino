//! A trace through every layer of the public API (issue #62): a ring on the
//! "target", its `TRINO_TRACE:` lines mixed into a noisy log, `collect`,
//! Chrome JSON out and back in, the statistics and a diff.

use trino_core::trace::{TracePlatform, TraceRing, track};
use trino_trace::{Diff, SessionStats, chrome, collect};

/// Frames of `tnl_us` T&L inside a render zone, 1 ms apart.
fn run(session: &str, tnl_us: u32, frames: u32) -> String {
    let mut r = TraceRing::<256>::new();
    let mut log = String::from("boot banner\n");
    let mut seq = 0;
    for f in 0..frames {
        let t = f * 1000;
        r.frame(t, f);
        r.open(track::CPU);
        r.zone("tnl", track::CPU, t + 10, t + 10 + tnl_us);
        r.close(track::CPU);
        r.zone("render", track::CPU, t, t + 20 + tnl_us);
        r.zone_at("scene", track::GPU, 0, t + 30, 200);
        r.counter("draws", track::CPU, t + 25 + tnl_us, 30 + f);
        // A packet every few frames, as the PC host flushes.
        if f % 3 == 2 {
            r.write_lines(TracePlatform::N64, session, &mut seq, &mut log)
                .unwrap();
            log.push_str("[ares] TRINO_STEP:something else\n");
        }
    }
    r.write_lines(TracePlatform::N64, session, &mut seq, &mut log)
        .unwrap();
    log
}

#[test]
fn a_log_becomes_perfetto_json_statistics_and_a_diff() {
    let log = format!("{}{}", run("n64-hub", 300, 10), run("pc-hub", 100, 10));
    let trace = collect(&log).unwrap();
    assert_eq!(trace.sessions.len(), 2);
    let n64 = trace.session("n64-hub").unwrap();
    assert_eq!(n64.platform, "n64");
    assert_eq!(n64.frames.len(), 10);
    assert_eq!(n64.tracks(), [track::CPU, track::GPU]);
    // Zones nest: tnl (depth 1) inside its frame's render.
    let (from, to) = n64.frame_range(4).unwrap();
    let inside: Vec<&str> = n64
        .zones_in(from, to)
        .filter(|z| z.track == track::CPU)
        .map(|z| n64.name_of(z.name))
        .collect();
    assert_eq!(inside, ["render", "tnl"]);

    // Chrome JSON round trip.
    let back = chrome::read(&chrome::write(&trace)).unwrap();
    assert_eq!(back, trace);

    let st = SessionStats::of(n64);
    let tnl = st.zone("tnl", track::CPU).unwrap();
    assert_eq!((tnl.count, tnl.mean_us, tnl.max_us), (10, 300.0, 300));
    assert_eq!(st.frames.mean_us, 1000.0);
    assert_eq!(st.counters[0].max, 39);

    let d = Diff::between(trace.session("pc-hub").unwrap(), n64);
    let z = d.zones.iter().find(|z| z.name == "tnl").unwrap();
    assert_eq!(z.mean_delta(), 200.0);
    assert_eq!(z.mean_pct(), Some(200.0));
}
